# Real Upstream Matrix Observability Specification

## 0. Status

- Version: `1.0.0`
- Scope: `tests/e2e/real-upstream-matrix.ts`
- Purpose: define the observable behavior required from the repository-local real-upstream matrix probe.

## 1. Nature of the probe

RUM-1. `tests/e2e/real-upstream-matrix.ts` is an observability probe against a real upstream path. It is not the repository's canonical protocol-correctness proof.

RUM-2. A failed scenario in this probe means at least one of the following is true:

1. Monoize request shaping regressed,
2. Monoize response adaptation regressed,
3. the upstream provider behavior drifted, or
4. the model did not follow the requested tool protocol strongly enough for the probe to observe the expected signal.

RUM-3. The probe MUST prefer deterministic prompts, deterministic tool-result payloads, and deterministic artifact capture so that a failure is inspectable after the run.

## 2. Artifact contract

RUM-4. For every executed scenario, the probe MUST write one request artifact under the configured output directory.

RUM-5. Each request artifact in RUM-4 MUST be JSON and MUST contain all of the following fields:

- `method`
- `path`
- `url`
- `headers`
- `body`

RUM-6. For every executed scenario, the probe MUST write the received response headers and raw response body.

RUM-7. For each tool roundtrip scenario (`tool_result_roundtrip` and `parallel_tool_roundtrip`) in each downstream family, the probe MUST additionally write an analysis artifact in JSON.

RUM-8. Each analysis artifact in RUM-7 MUST contain all of the following:

- the original user instruction used for the scenario,
- the deterministic expected final answer string,
- the deterministic required substrings,
- the deterministic tool-result payload strings,
- the final downstream text surface extracted by the probe,
- the evaluation mode chosen by the probe,
- the count of observed required substrings.

## 3. Tool-roundtrip prompting and second-leg context

RUM-9. In the first-leg single-tool scenario, the user instruction MUST require exactly one weather tool call for Taipei and MUST instruct the model that, after tool results are provided, the final answer must be the exact plain-text answer string required by this spec.

RUM-10. In the first-leg parallel-tool scenario, the user instruction MUST require exactly two tool calls in one assistant turn: one `weather` call for Taipei and one `websearch` call for Monoize.

RUM-11. The instruction in RUM-10 MUST additionally require that, after tool results are provided, the final answer be the exact plain-text answer string required by this spec.

RUM-12. Every second-leg tool-result request MUST preserve the original user instruction from the corresponding first leg.

RUM-13. Every second-leg tool-result request MUST also preserve the tool-call surface emitted by the first leg and the deterministic tool-result payloads supplied by the probe.

## 4. Deterministic probe payloads

RUM-14. The single-tool roundtrip payload string MUST be `WEATHER_RESULT__TAIPEI__SUNNY_25C__MONOIZE_SENTINEL`.

RUM-15. The parallel weather payload string MUST be `WEATHER_RESULT__TAIPEI__SUNNY_25C__MONOIZE_SENTINEL`.

RUM-16. The parallel websearch payload string MUST be `WEBSEARCH_RESULT__MONOIZE__PROXY__MONOIZE_SENTINEL`.

RUM-17. The expected final answer string for the single-tool roundtrip MUST be exactly:

```text
FINAL_ANSWER weather=WEATHER_RESULT__TAIPEI__SUNNY_25C__MONOIZE_SENTINEL
```

RUM-18. The expected final answer string for the parallel-tool roundtrip MUST be exactly:

```text
FINAL_ANSWER weather=WEATHER_RESULT__TAIPEI__SUNNY_25C__MONOIZE_SENTINEL websearch=WEBSEARCH_RESULT__MONOIZE__PROXY__MONOIZE_SENTINEL
```

## 5. Observable evaluation rules

RUM-19. A tool roundtrip scenario MUST be marked `ok = true` only when the HTTP status is `200` and at least one of the following is true for the extracted final text surface:

1. the normalized text is exactly the expected final answer string, or
2. the normalized text contains every required substring for that scenario.

RUM-20. If RUM-19 succeeds by rule 1, the evaluation mode MUST be recorded as `exact-answer`.

RUM-21. If RUM-19 succeeds by rule 2, the evaluation mode MUST be recorded as `substring-match`.

RUM-22. If none of the RUM-19 rules succeed, the evaluation mode MUST be recorded as `missing-required-substrings`.

RUM-23. For tool-roundtrip analysis artifacts, the probe MUST record `required_substring_hits` and the normalized final text preview used for inspection.
