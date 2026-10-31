# ADR：地址栏输入的 URL 规范化（无 scheme 补 `http://`）

日期：2026-10-31（K3 导航与网络完备轮 T5）

## 决策

地址栏提交的输入（`BrowserApp::navigate` 的地址栏路径）在入历史栈与抓取前做
一步规范化，实现为 `nexty-chrome::resources::normalize_address_bar_input`：

1. **无 scheme 补 `http://`**（HTTP-first）：输入不以 WHATWG URL 的 scheme
   前缀（`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`）开头时，前面拼上
   `http://`。用户输入 `example.com` 实际导航 `http://example.com/`。
2. **带合法 scheme 的输入原样返回**：`https://…`、`about:blank`、`mailto:…`
   各自走到其语义终点——`about:` 在 `load` 入口特判为内部空白页；其余非
   `http(s)` scheme 由抓取后端的协议白名单判 `InvalidUrl`，进既有错误页。
3. **合法性不前置校验**：规范化只补 scheme、不做 URL 解析校验。非法输入
   （如含空格的 `hello world`）走既有抓取管线，由错误页兜底——错误页路径
   已包含生命周期合成、历史语义与错误串收敛，不引入第二条错误路径。

## 为什么不是别的

- **不猜测 `www.` 前缀**：`example.com` → `http://example.com/` 即可，
  Chromium 的 `www.` 自动补全是历史兼容行为，猜测错误时的行为更难解释。
- **不在 http/https 间猜测升级**：HTTP-first 简单可预期；HSTS / HTTPS-first
  升级策略留给后续轮。
- **不用正则 crate 判 scheme**：判定是「首字母 + 字符集 + 冒号」的几行字符
  扫描，引入 `regex` 违反依赖最小化。
- **不前置 `Url::parse` 校验**：校验点前移需要手工构建第二条错误页路径
  （历史栈、生命周期、转义），而抓取路径的 `InvalidUrl → 「无效的地址」 →
  错误页` 已完整存在，复用即可。

## 边界与已知偏差

- **不支持 host:port 简写**：`localhost:8080` 的 `localhost:` 恰好匹配 scheme
  语法前缀，会按 scheme=localhost 解析进错误页；需要显式输入
  `http://localhost:8080`。（Chromium 有 host 猜测启发式，本轮不引入。）
- 规范化作用于历史栈入口：栈里存的是规范化后的 URL，后退/前进重放的即
  实际导航目标。
- 页面链接点击不走此规范化：链接 href 已由 `resolve` 按文档 base 绝对化
  （带 scheme），规范化对其是无操作。
