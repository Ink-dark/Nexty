# UAX#14 / Unicode 属性数据（Unicode 18.0.0）

`src/inline.rs` 的断行引擎在运行时通过 `include_str!` 读取这些文件，并用
`OnceLock` 解析一次（无 build script、无生成代码）。`parse_tables` 会断言版本
字符串仍为 `18.0.0`；升级语料时必须同时更新这里与
`tests/data/README.md`，并重跑 `tests/inline_conformance.rs`。

| 文件 | 来源 | SHA-256 |
| --- | --- | --- |
| `DerivedLineBreak-18.0.0.txt` | <https://www.unicode.org/Public/18.0.0/ucd/extracted/DerivedLineBreak.txt> | `e015a6d4bde8dd0f8b72bf15ac9a4f588146ce6bc0bf8980e9dae8302cdbdc28` |
| `EastAsianWidth-18.0.0.txt` | <https://www.unicode.org/Public/18.0.0/ucd/EastAsianWidth.txt> | `a0cf29eacd00cfcaec4381c6b7c281685f18dbb4e7ff82b4076ccb342ca839aa` |
| `PiPf-18.0.0.txt` | 从 `DerivedGeneralCategory.txt` 提取 `Pi` / `Pf` 区间 | `c9bba0940097c11a2b6abe380cb866f2defc4d0b09469f1f861ce0d39b780309` |
| `Combining-18.0.0.txt` | 从 `DerivedGeneralCategory.txt` 提取 `Mn` / `Mc` / `Me` 区间 | `5d4abf83e40d558326e01b4769b758f2860949d8e59e9e4bf4d0a099a89f2122` |
| `ExtPict-18.0.0.txt` | 从 `emoji-data.txt` 提取 `Extended_Pictographic` 区间 | `56e8d16adc2ffda89e90d432464887fff06e471b8670ac3c5422c6a344f89648` |
| `Unassigned-18.0.0.txt` | 从 `DerivedGeneralCategory.txt` 提取 `Cn` 区间 | `52cd7af4aea473981713deb7070afa1fcf27f435f56fe52aec0ff31f0a709e99` |

许可：Unicode License（各文件头保留版权与使用条款；本仓 `deny.toml` 的许可证
白名单含 `Unicode-3.0`）。

## 为什么自研 UAX#14 引擎

`unicode-linebreak` 0.1.5 的 Unicode 数据停在 15.0，对 Unicode 18.0.0 官方
`LineBreakTest.txt` 有 1,117 条差异（新增 HH 类、AK/AP/AS/VF/VI、LB12a/LB15c/
LB15d/LB19a/LB25/LB28a 等规则）。ICU4X 2.3 的 Unicode 17 行分段仍有 30 条
差异（同样集中在 Unicode 18 的规则/属性变化）。因此按 UAX#14（Unicode
18.0.0）规则直接实现，类数据取官方 UCD 文件；`tests/inline_conformance.rs`
以官方 19,346 条语料逐条比对。
