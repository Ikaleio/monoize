use super::*;

impl UserStore {
    pub async fn has_plan_capacity_for_groups(
        &self,
        user_id: &str,
        api_key_id: Option<&str>,
        billing_group_ids: &[String],
        pending_nano_usd: i128,
    ) -> Result<bool, String> {
        if api_key_id.is_none() || billing_group_ids.is_empty() {
            return Ok(false);
        }
        let now = Utc::now();
        let read = self.db.read();
        let Some(subscription) = self
            .active_subscription_on(read, user_id, now, false)
            .await?
        else {
            return Ok(false);
        };
        if !billing_group_ids
            .iter()
            .any(|group| subscription.group_ids.contains(group))
        {
            return Ok(false);
        }
        let usage = self.usage_snapshot_on(read, &subscription, now).await?;
        let remaining = Self::remaining_capacity(&subscription, usage.sums)?;
        let pending = subscription
            .multiplier
            .checked_scale_i128(pending_nano_usd.max(0))
            .ok_or_else(|| "pending plan charge overflow".to_string())?;
        Ok(remaining > pending)
    }

    pub(crate) async fn allocate_plan_charge_tx(
        &self,
        tx: &DatabaseTransaction,
        request: PlanChargeRequest<'_>,
    ) -> Result<PlanChargeAllocation, BillingError> {
        let PlanChargeRequest {
            user_id,
            api_key_id,
            billing_group_id,
            request_id,
            settled_charge_nano_usd,
            now,
        } = request;
        if settled_charge_nano_usd <= 0 {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: 0,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: 0,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        let Some(api_key_id) = api_key_id else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        let Some(group_id) = billing_group_id else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        let subscription = self
            .active_subscription_on(tx, user_id, now, true)
            .await
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let Some(subscription) = subscription else {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        };
        if !subscription.group_ids.iter().any(|id| id == group_id) {
            return Ok(PlanChargeAllocation {
                adjusted_charge_nano_usd: settled_charge_nano_usd,
                plan_covered_nano_usd: 0,
                fallback_nano_usd: settled_charge_nano_usd,
                subscription_id: None,
                plan_id: None,
                multiplier: None,
            });
        }
        let adjusted = subscription
            .multiplier
            .checked_scale_i128(settled_charge_nano_usd)
            .ok_or_else(|| {
                BillingError::new(BillingErrorKind::Overflow, "plan multiplier overflow")
            })?;
        let usage = self
            .usage_snapshot_on(tx, &subscription, now)
            .await
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let remaining = Self::remaining_capacity(&subscription, usage.sums)
            .map_err(|error| BillingError::new(BillingErrorKind::Internal, error))?;
        let covered = adjusted.min(remaining);
        let fallback = adjusted.checked_sub(covered).ok_or_else(|| {
            BillingError::new(BillingErrorKind::Overflow, "plan allocation overflow")
        })?;
        if covered > 0 {
            let internal = |error: String| BillingError::new(BillingErrorKind::Internal, error);
            let cumulative_overflow =
                || BillingError::new(BillingErrorKind::Overflow, "plan usage cumulative overflow");
            let usage_id = uuid::Uuid::new_v4().to_string();
            let created_at = now.to_rfc3339();
            let order_key = || -> Vec<SeaValue> {
                vec![
                    subscription.id.clone().into(),
                    created_at.clone().into(),
                    usage_id.clone().into(),
                ]
            };
            let preceding = tx
                .query_one(self.db.stmt(
                    "SELECT cumulative_nano_usd FROM billing_plan_usage WHERE subscription_id = $1 AND (created_at, id) < ($2, $3) ORDER BY created_at DESC, id DESC LIMIT 1",
                    order_key(),
                ))
                .await
                .map_err(|error| internal(error.to_string()))?
                .as_ref()
                .map(parse_usage_cumulative)
                .transpose()
                .map_err(internal)?
                .unwrap_or(0);
            let cumulative = preceding
                .checked_add(covered)
                .ok_or_else(cumulative_overflow)?;
            // BP-D11: empty unless the clock stepped back or `created_at` tied with a
            // greater id; those later rows already count in their own prefix sums.
            let successors = tx
                .query_all(self.db.stmt(
                    "SELECT id, cumulative_nano_usd FROM billing_plan_usage WHERE subscription_id = $1 AND (created_at, id) > ($2, $3)",
                    order_key(),
                ))
                .await
                .map_err(|error| internal(error.to_string()))?
                .iter()
                .map(|row| {
                    let id = row.try_get::<String>("", "id").map_err(|error| internal(sql_err(error)))?;
                    let shifted = parse_usage_cumulative(row)
                        .map_err(internal)?
                        .checked_add(covered)
                        .ok_or_else(cumulative_overflow)?;
                    Ok((id, shifted))
                })
                .collect::<Result<Vec<_>, BillingError>>()?;
            tx.execute(self.db.stmt(
                "INSERT INTO billing_plan_usage (id, subscription_id, user_id, api_key_id, request_id, group_id, amount_nano_usd, cumulative_nano_usd, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                vec![
                    usage_id.into(),
                    subscription.id.clone().into(),
                    user_id.into(),
                    api_key_id.into(),
                    request_id.into(),
                    group_id.into(),
                    covered.to_string().into(),
                    cumulative.to_string().into(),
                    created_at.into(),
                ],
            ))
            .await
            .map_err(|error| internal(error.to_string()))?;
            for (id, shifted) in successors {
                tx.execute(self.db.stmt(
                    "UPDATE billing_plan_usage SET cumulative_nano_usd = $1 WHERE id = $2",
                    vec![shifted.to_string().into(), id.into()],
                ))
                .await
                .map_err(|error| internal(error.to_string()))?;
            }
        }
        Ok(PlanChargeAllocation {
            adjusted_charge_nano_usd: adjusted,
            plan_covered_nano_usd: covered,
            fallback_nano_usd: fallback,
            subscription_id: Some(subscription.id),
            plan_id: Some(subscription.plan_id),
            multiplier: Some(subscription.multiplier.canonical()),
        })
    }
}
