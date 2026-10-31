# 测试字体（仅测试资产，非 cargo 依赖）

Space Mono（Regular / Bold），SIL Open Font License 1.1（见同目录 `OFL.txt`）。

- 来源：<https://github.com/google/fonts/tree/main/ofl/spacemono>
- 快照 commit：`8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5`（2026-10-31 下载）
- 用途：crates/nexty-text 的 @font-face 注册表（`FontRegistry`）集成测试——
  经描述符声明为测试族名（CSS 声明名与字体内部族名解耦），供
  weight/style 就近匹配与整形归属断言使用。
- 合规说明：本字体是随仓库分发的测试资产而非 cargo 依赖，不在
  deny.toml 依赖白名单管辖范围；OFL 许可合规记录进 K4 轮 T4 ADR。
