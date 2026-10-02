# Nexty
Muskitty 不造轮子分支

## CI / CD

仓库通过 GitHub Actions 进行持续集成与发布，行为基准见 [`AGENTS.md`](AGENTS.md)。

- **CI**（`.github/workflows/ci.yml`）：推送 `main` 或开 PR 时运行
  `cargo fmt --check`、`cargo clippy -D warnings`、三平台（Linux/macOS/Windows）
  `cargo test`、`cargo deny check`、`cargo about` 与 MSRV(1.90) 校验。
- **Release**（`.github/workflows/release.yml`）：推送 `v*` 标签（或手动
  `workflow_dispatch`）时，在原生 runner 上构建 `nexty` 二进制（Linux x86_64 /
  Windows x86_64），打包为 `tar.gz` 并附 `sha256`，连同 `dependencies.html`
  许可证清单一起发布为 GitHub Release。

### 发版步骤

```bash
# 1. 在 main 上完成改动并合并
# 2. 打标签并推送
git tag v0.1.0
git push origin v0.1.0
# 3. 流水线自动构建制品并创建 GitHub Release
```

## 本地构建

```bash
cargo run -p nexty-chrome --bin nexty   # 启动浏览器外壳
cargo test --workspace                   # 运行全部测试
```
