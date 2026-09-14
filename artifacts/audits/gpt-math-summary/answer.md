# GPT 数学题实测

渠道：LGS-Codex。模型：gpt-5.6-sol。请求参数：effort=high，summary=detailed。

## 原始概要

**Planning convexity-based inequality proof**

**Developing algebraic substitution proof approach**

**Analyzing inequalities using product constraints**

**Formulating algebraic inequality transformations**

**Assessing inequality constraints for homogeneity**

**Verifying Muirhead inequality with test cases**

**Verifying weighted AM-GM inequality**

**Planning derivative-based convexity proof**

**Verifying Chebyshev covariance identity**

**Refining AM-GM inequality application**

**Verifying derivative-based inequality equality conditions**

**Formulating calculus-based AM-GM proof with equality**

## 模型回答

**证明一：加权 AM-GM 不等式**

由 \(abc=1\)，有
\[
a^2=a^3bc,\qquad b^2=ab^3c,\qquad c^2=abc^3.
\]
因此只需证明
\[
a^5+b^5+c^5\ge a^3bc+ab^3c+abc^3.
\]

由加权 AM-GM 不等式，
\[
\frac35a^5+\frac15b^5+\frac15c^5
\ge (a^5)^{3/5}(b^5)^{1/5}(c^5)^{1/5}
=a^3bc,
\]
即
\[
3a^5+b^5+c^5\ge 5a^3bc.
\]
循环交换 \(a,b,c\)，还可得到
\[
a^5+3b^5+c^5\ge 5ab^3c,
\]
\[
a^5+b^5+3c^5\ge 5abc^3.
\]
将三式相加，得到
\[
5(a^5+b^5+c^5)
\ge 5(a^3bc+ab^3c+abc^3).
\]
故
\[
a^5+b^5+c^5\ge a^2+b^2+c^2.
\]

---

**证明二：对数代换与导数法**

令
\[
x=\ln a,\qquad y=\ln b,\qquad z=\ln c.
\]
由于 \(abc=1\)，所以
\[
x+y+z=0.
\]

定义
\[
F(t)=e^{tx}+e^{ty}+e^{tz}\qquad (t\ge 0).
\]
则
\[
F(5)=a^5+b^5+c^5,\qquad F(2)=a^2+b^2+c^2.
\]

下面证明 \(F(t)\) 在 \(t\ge0\) 上单调不减。注意
\[
F'(t)=xe^{tx}+ye^{ty}+ze^{tz}.
\]
利用 \(x+y+z=0\)，有
\[
\begin{aligned}
3F'(t)
={}&(x-y)(e^{tx}-e^{ty})\\
&+(x-z)(e^{tx}-e^{tz})\\
&+(y-z)(e^{ty}-e^{tz}).
\end{aligned}
\]
当 \(t\ge0\) 时，函数 \(u\mapsto e^{tu}\) 单调不减，因此每一项都满足
\[
(x-y)(e^{tx}-e^{ty})\ge0,
\]
以及类似的不等式。于是
\[
F'(t)\ge0\qquad (t\ge0).
\]
所以
\[
F(5)\ge F(2),
\]
即
\[
\boxed{a^5+b^5+c^5\ge a^2+b^2+c^2}.
\]

此证明只使用了对数代换、导数以及单调性，没有使用 Jensen 不等式或幂平均不等式。

---

**等号情况**

在第一种证明中，加权 AM-GM 取等要求
\[
a^5=b^5=c^5,
\]
由于 \(a,b,c>0\)，故
\[
a=b=c.
\]
结合 \(abc=1\)，得到
\[
a=b=c=1.
\]

在第二种证明中，若不全相等，则对于每个 \(t>0\)，上面的三个差积中至少有一个严格为正，从而 \(F'(t)>0\)，于是
\[
F(5)>F(2).
\]
因此等号同样只在
\[
\boxed{a=b=c=1}
\]
时成立。
