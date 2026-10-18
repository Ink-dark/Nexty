# nexty-layout 测试语料

## `LineBreakTest-18.0.0.txt`

- 来源：<https://www.unicode.org/Public/18.0.0/ucd/auxiliary/LineBreakTest.txt>
- 版本：Unicode 18.0.0（文件头 `LineBreakTest-18.0.0.txt`，日期 2026-08-07）
- SHA-256：`fd9ff9eca411080085b41471bba79ae19d9db2e16924f9770b8509f1affd3f95`
- 许可：Unicode License（见文件头；本仓 `deny.toml` 白名单含 `Unicode-3.0`）

这是 UAX #14（Unicode Line Breaking Algorithm）的官方默认测试语料。每行格式为：

```
× 2757 × 0457 ÷	#  × [2.0] ... × [9.0] ... ÷ [3.0]
```

- `÷`：该位置存在断行机会（含强制断行）；`×`：禁止断行。
- `[...]`：决定该位置的 UAX #14 规则编号（`2.0` = start of text，`3.0` =
  end of text，其余对应 LB4–LB31 及其子规则）。
- 本文件按 Unicode 版本钉死；升级语料时必须同时更新 `inline_conformance.rs`
  里的版本断言与 SHA-256。

测试 **不修改** 语料，只在运行时读取。`tests/inline_conformance.rs` 依据注释里的
规则编号把用例归入 LB4–8 / LB9–10 / LB21–31 三组，并另跑一遍全量语料。
