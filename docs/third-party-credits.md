# 第三方依赖致谢名单

本项目使用了大量 Rust 生态的开源 crate。在此向所有开源作者致谢——没有这些工作，Nexty 不可能存在。以下名单按**许可证类型**分组，每项列出 crate 名称、版权年份与版权人。

## 阅读说明

- **分组依据**：按各 crate SPDX 声明中出现的许可证标识集合归组。SPDX 表达式写法差异很大（`Apache-2.0 OR MIT`、`MIT OR Apache-2.0`、`MIT/Apache-2.0` 语义相同），故按集合归组，避免出现几十个语义重复的章节。
- **原始表达式**：表格中的「SPDX 声明」列如实列出各 crate 的原始写法，信息无损。需要逐字原文时请查 `docs/dependencies.html`。
- **双许可（OR）**：可任选其一履行，本项目未对同一 crate 同时履行多项。
- **多许可并存（AND，含 LLVM 例外等）**：须同时履行各项义务。此类 crate 在下表中的 SPDX 声明列会显示完整表达式。
- **年份**：优先取 crate 随附 LICENSE 文件中的版权声明；部分 crate 未声明年份，标注「推定」者为 crates.io 首次发布年份，仅供参考。
- **版权人**：优先取 crate 的 `authors` 元数据；无该字段时取 LICENSE 中的版权声明；两者皆无时标注「见该 crate 仓库的版权声明」。
- 名单共 **395 个第三方 crate**（不含本项目 workspace 内的 8 个 `nexty-*` crate）。生成方式见文末。

## 许可证概览

| 许可证 | 全称 | crate 数 |
| --- | --- | ---: |
| `MIT` | MIT License（含 MIT + Apache-2.0 + 0BSD） | 339 |
| `Apache-2.0` | Apache License 2.0 | 15 |
| `BSD-2-Clause` | BSD 2-Clause Simplified | 1 |
| `BSD-3-Clause` | BSD 3-Clause New/Revised（含 BSD-3-Clause + (Apache-2.0 + MIT)） | 4 |
| `ISC` | ISC License（含 ISC + (Apache-2.0 + ISC)） | 4 |
| `Zlib` | zlib License | 3 |
| `MPL-2.0` | Mozilla Public License 2.0 | 4 |
| `Unicode-3.0` | Unicode License v3 | 23 |
| `CC0-1.0` | Creative Commons Zero v1.0 Universal | 1 |
| `CDLA-Permissive-2.0` | Community Data License Agreement Permissive 2.0 | 1 |

## MIT License（`MIT`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `adler2` | 2.0.1 | 2025（推定） | Jonas Schievink 等 2 位贡献者 | `0BSD OR MIT OR Apache-2.0` |
| `ahash` | 0.8.12 | 2018 | Tom Kaitchuck | `MIT OR Apache-2.0` |
| `allocator-api2` | 0.2.21 | 2024（推定） | Zakarum | `MIT OR Apache-2.0` |
| `android-activity` | 0.6.1 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `android-properties` | 0.2.2 | 2020 | Mikhail Lappo | `MIT` |
| `android_system_properties` | 0.1.6 | 2013、2016 | Nicolas Silva | `MIT OR Apache-2.0` |
| `arrayvec` | 0.7.8 | 2026（推定） | bluss | `MIT OR Apache-2.0` |
| `as-raw-xcb-connection` | 1.0.1 | 2019 | as-raw-xcb-connection Contributers | `MIT OR Apache-2.0` |
| `ash` | 0.38.0+1.3.281 | 2016 | Maik Klein 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `atomic-waker` | 1.1.2 | 2016、2017 | Stjepan Glavina | `Apache-2.0 OR MIT` |
| `autocfg` | 1.5.1 | 2018 | Josh Stone | `Apache-2.0 OR MIT` |
| `aws-lc-sys` | 0.45.0 | 2014-2024 | AWS-LC | `ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)` |
| `base64` | 0.23.1 | 2025 | Marshall Pierce | `MIT OR Apache-2.0` |
| `bit-set` | 0.9.1 | 2026 | Alexis Beingessner | `Apache-2.0 OR MIT` |
| `bit-vec` | 0.9.1 | 2023 | Alexis Beingessner | `Apache-2.0 OR MIT` |
| `bitflags` | 2.13.2 | 2014 | The Rust Project Developers | `MIT OR Apache-2.0` |
| `block2` | 0.6.2 | 2025（推定） | Mads Marquart | `MIT` |
| `bumpalo` | 3.20.3 | 2019 | Nick Fitzgerald | `MIT OR Apache-2.0` |
| `bytemuck` | 1.25.2 | 2019 | Lokathor | `Zlib OR Apache-2.0 OR MIT` |
| `bytemuck_derive` | 1.12.1 | 2019 | Lokathor | `Zlib OR Apache-2.0 OR MIT` |
| `byteorder-lite` | 0.1.0 | 2015 | Andrew Gallant | `Unlicense OR MIT` |
| `bytes` | 1.12.1 | 2018 | Carl Lerche 等 2 位贡献者 | `MIT` |
| `calloop` | 0.13.0 | 2018 | Elinor Berger | `MIT` |
| `calloop-wayland-source` | 0.3.0 | 2023 | Kirill Chibisov | `MIT` |
| `cc` | 1.5.1 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `cfg-if` | 1.0.5 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `cfg_aliases` | 0.2.2 | 2020 | Zicklag | `MIT` |
| `chacha20` | 0.10.2 | 2019-2026 | RustCrypto Developers | `MIT OR Apache-2.0` |
| `cmake` | 0.1.58 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `color` | 0.3.3 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `color_quant` | 1.1.0 | 2016 | nwin | `MIT` |
| `combine` | 4.6.8 | 2015 | Markus Westerlind | `MIT` |
| `concurrent-queue` | 2.5.0 | 2024（推定） | Stjepan Glavina 等 3 位贡献者 | `Apache-2.0 OR MIT` |
| `core-foundation` | 0.10.1 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `core-foundation-sys` | 0.8.7 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `core-graphics` | 0.23.2 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `core-graphics-types` | 0.1.3 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `core_detect` | 1.0.0 | 2017-2020 | Thom Chiovoloni | `MIT/Apache-2.0` |
| `cpufeatures` | 0.3.1 | 2020-2026 | RustCrypto Developers | `MIT OR Apache-2.0` |
| `crc32fast` | 1.5.2 | 2018 | Sam Rijs 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `crossbeam-utils` | 0.8.23 | 2019 | The Crossbeam Project Developers | `MIT OR Apache-2.0` |
| `crunchy` | 0.2.4 | 2017-2023 | Eira Fransham | `MIT` |
| `cursor-icon` | 1.2.0 | 2023 | Kirill Chibisov | `MIT OR Apache-2.0 OR Zlib` |
| `derive_more` | 2.1.1 | 2016 | Jelte Fennema | `MIT` |
| `derive_more-impl` | 2.1.1 | 2016 | Jelte Fennema | `MIT` |
| `dispatch` | 0.2.0 | 2020（推定） | Steven Sheldon | `MIT` |
| `dispatch2` | 0.3.1 | 2026（推定） | Mads Marquart 等 2 位贡献者 | `Zlib OR Apache-2.0 OR MIT` |
| `displaydoc` | 0.2.7 | 2026（推定） | Jane Lusby | `MIT OR Apache-2.0` |
| `dlib` | 0.5.3 | 2015 | Elinor Berger | `MIT` |
| `document-features` | 0.2.12 | 2020 | Slint Developers | `MIT OR Apache-2.0` |
| `downcast-rs` | 1.2.1 | 2020 | Ashish Myles 等 2 位贡献者 | `MIT/Apache-2.0` |
| `dpi` | 0.1.2 | 2018 | Jorge Aparicio | `Apache-2.0 AND MIT` |
| `dtoa` | 1.0.11 | 2025（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `equivalent` | 1.0.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `errno` | 0.3.14 | 2014 | Chris Wong 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `euclid` | 0.22.14 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `fastrand` | 2.5.0 | 2026（推定） | Stjepan Glavina | `Apache-2.0 OR MIT` |
| `fdeflate` | 0.3.7 | 2024（推定） | The image-rs Developers | `MIT OR Apache-2.0` |
| `fearless_simd` | 0.4.1 | 2018 | Raph Levien | `Apache-2.0 OR MIT` |
| `find-msvc-tools` | 0.1.14 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `flate2` | 1.1.10 | 2014-2026 | Alex Crichton 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `fnv` | 1.0.7 | 2017 | Alex Crichton | `Apache-2.0 / MIT` |
| `font-types` | 0.12.5 | 2019 | Fontations Developers | `MIT OR Apache-2.0` |
| `fontique` | 0.11.1 | 2024 | the Parley Authors | `Apache-2.0 OR MIT` |
| `foreign-types` | 0.5.0 | 2017 | Steven Fackler | `MIT/Apache-2.0` |
| `foreign-types-macros` | 0.2.4 | 2017 | Steven Fackler | `MIT/Apache-2.0` |
| `foreign-types-shared` | 0.3.1 | 2017 | Steven Fackler | `MIT/Apache-2.0` |
| `form_urlencoded` | 1.2.2 | 2013-2016 | The rust-url developers | `MIT OR Apache-2.0` |
| `fs_extra` | 1.3.0 | 2017 | Denis Kurilenko | `MIT` |
| `futures-channel` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `futures-core` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `futures-io` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `futures-sink` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `futures-task` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `futures-util` | 0.3.34 | 2016、2017 | Alex Crichton；The Tokio Authors | `MIT OR Apache-2.0` |
| `getrandom` | 0.4.3 | 2014、2018-2026 | The Rand Project Developers | `MIT OR Apache-2.0` |
| `gif` | 0.14.2 | 2015 | The image-rs Developers | `MIT OR Apache-2.0` |
| `glifo` | 0.3.0 | 2025 | the Vello Authors | `Apache-2.0 OR MIT` |
| `glow` | 0.17.0 | 2026（推定） | Joshua Groves 等 2 位贡献者 | `MIT OR Apache-2.0 OR Zlib` |
| `gpu-allocator` | 0.28.0 | 2021 | Traverse Research | `MIT OR Apache-2.0` |
| `gpu-descriptor` | 0.3.2 | 2025（推定） | Zakarum | `MIT OR Apache-2.0` |
| `gpu-descriptor-types` | 0.2.0 | 2024（推定） | Zakarum | `MIT OR Apache-2.0` |
| `guillotiere` | 0.7.0 | 2019 | Nicolas Silva | `MIT/Apache-2.0` |
| `h2` | 0.4.19 | 2017 | Carl Lerche 等 2 位贡献者 | `MIT` |
| `half` | 2.7.1 | 2025（推定） | Kathryn Long | `MIT OR Apache-2.0` |
| `harfrust` | 0.12.0 | 2020 | Yevhenii Reizner | `MIT` |
| `hashbrown` | 0.17.1 | 2016 | Amanieu d'Antras | `MIT OR Apache-2.0` |
| `hermit-abi` | 0.5.3 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `html5ever` | 0.40.1 | 2014 | The html5ever Project Developers | `MIT OR Apache-2.0` |
| `http` | 1.5.0 | 2017 | Alex Crichton 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `http-body` | 1.1.0 | 2019-2026 | Carl Lerche 等 3 位贡献者 | `MIT` |
| `http-body-util` | 0.1.5 | 2019-2026 | Carl Lerche 等 3 位贡献者 | `MIT` |
| `httparse` | 1.10.1 | 2015-2025 | Sean McArthur | `MIT OR Apache-2.0` |
| `hyper` | 1.11.1 | 2014-2026 | Sean McArthur | `MIT` |
| `hyper-rustls` | 0.27.10 | 2016 | Joseph Birr-Pixton <jpixton@gmail.com> | `Apache-2.0 OR ISC OR MIT` |
| `hyper-util` | 0.1.21 | 2023-2025 | Sean McArthur | `MIT` |
| `idna` | 1.1.0 | 2013-2025 | The rust-url developers | `MIT OR Apache-2.0` |
| `idna_adapter` | 1.2.2 | 2026（推定） | The rust-url developers | `Apache-2.0 OR MIT` |
| `image` | 0.25.10 | 2026（推定） | The image-rs Developers | `MIT OR Apache-2.0` |
| `image-webp` | 0.2.4 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `indexmap` | 2.14.2 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `ipnet` | 2.12.2 | 2017 | Kris Price | `MIT OR Apache-2.0` |
| `itoa` | 1.0.18 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `jni` | 0.22.4 | 2026（推定） | jni team | `MIT OR Apache-2.0` |
| `jni-macros` | 0.22.4 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `jni-sys` | 0.4.1 | 2015 | Steven Fackler 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `jni-sys-macros` | 0.4.1 | 2026（推定） | Robert Bragg | `MIT OR Apache-2.0` |
| `jobserver` | 0.1.35 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `js-sys` | 0.3.106 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `khronos-egl` | 6.0.0 | 2023（推定） | Timothée Haudebourg 等 2 位贡献者 | `MIT/Apache-2.0` |
| `kurbo` | 0.13.1 | 2018 | Raph Levien | `Apache-2.0 OR MIT` |
| `libc` | 0.2.189 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `libm` | 0.2.16 | 2026（推定） | Alex Crichton 等 4 位贡献者 | `MIT` |
| `libredox` | 0.1.25 | 2023 | 4lDO2 | `MIT` |
| `linebender_resource_handle` | 0.1.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `linux-raw-sys` | 0.12.1 | 2025（推定） | Dan Gohman | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `litrs` | 1.0.0 | 2020 | Lukas Kalbertodt | `MIT OR Apache-2.0` |
| `lock_api` | 0.4.14 | 2016 | Amanieu d'Antras | `MIT OR Apache-2.0` |
| `log` | 0.4.34 | 2014 | The Rust Project Developers | `MIT OR Apache-2.0` |
| `lru-slab` | 0.1.3 | 2024 | Benjamin Saunders | `MIT OR Apache-2.0 OR Zlib` |
| `markup5ever` | 0.40.0 | 2014 | The html5ever Project Developers | `MIT OR Apache-2.0` |
| `memchr` | 2.8.3 | 2015 | Andrew Gallant 等 2 位贡献者 | `Unlicense OR MIT` |
| `memmap2` | 0.9.11 | 2015、2020 | Dan Burkert 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `mime` | 0.3.17 | 2014 | Sean McArthur | `MIT OR Apache-2.0` |
| `miniz_oxide` | 0.9.1 | 2010-2014、2013-2014、2017、2017-2024、2020 | Frommi 等 3 位贡献者 | `MIT OR Zlib OR Apache-2.0` |
| `mio` | 1.2.3 | 2014 | Carl Lerche 等 3 位贡献者 | `MIT` |
| `multiversion_no_op` | 1.0.0 | 2026（推定） | Henri Sivonen | `Apache-2.0 OR MIT` |
| `naga` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `ndk` | 0.9.0 | 2024（推定） | The Rust Mobile contributors | `MIT OR Apache-2.0` |
| `ndk-context` | 0.1.1 | 2022（推定） | The Rust Windowing contributors | `MIT OR Apache-2.0` |
| `ndk-sys` | 0.6.0+11769913 | 2024（推定） | The Rust Windowing contributors | `MIT OR Apache-2.0` |
| `new_debug_unreachable` | 1.0.6 | 2015 | Matt Brubeck 等 2 位贡献者 | `MIT` |
| `num-traits` | 0.2.19 | 2014 | The Rust Project Developers | `MIT OR Apache-2.0` |
| `num_enum` | 0.7.6 | 2018 | Daniel Wagner-Hall 等 3 位贡献者 | `BSD-3-Clause OR MIT OR Apache-2.0` |
| `num_enum_derive` | 0.7.6 | 2018 | Daniel Wagner-Hall 等 3 位贡献者 | `BSD-3-Clause OR MIT OR Apache-2.0` |
| `objc-sys` | 0.3.5 | 2024（推定） | Mads Marquart | `MIT` |
| `objc2` | 0.6.4 | 2026（推定） | Mads Marquart | `MIT` |
| `objc2-app-kit` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-cloud-kit` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-contacts` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-core-data` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-core-foundation` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `Zlib OR Apache-2.0 OR MIT` |
| `objc2-core-image` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-core-location` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-core-text` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `Zlib OR Apache-2.0 OR MIT` |
| `objc2-encode` | 4.1.0 | 2025（推定） | Mads Marquart | `MIT` |
| `objc2-foundation` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-link-presentation` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-metal` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `Zlib OR Apache-2.0 OR MIT` |
| `objc2-quartz-core` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `Zlib OR Apache-2.0 OR MIT` |
| `objc2-symbols` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-ui-kit` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-uniform-type-identifiers` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `objc2-user-notifications` | 0.2.2 | 2024（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `once_cell` | 1.21.4 | 2026（推定） | Aleksey Kladov | `MIT OR Apache-2.0` |
| `openssl-probe` | 0.2.1 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `orbclient` | 0.3.55 | 2015-2019 | Jeremy Soller | `MIT` |
| `ordered-float` | 5.5.0 | 2015 | Jonathan Reem 等 2 位贡献者 | `MIT` |
| `parking_lot` | 0.12.5 | 2016 | Amanieu d'Antras | `MIT OR Apache-2.0` |
| `parking_lot_core` | 0.9.12 | 2016 | Amanieu d'Antras | `MIT OR Apache-2.0` |
| `parlance` | 0.1.0 | 2020 | the Parley Authors | `Apache-2.0 OR MIT` |
| `parley` | 0.11.1 | 2020 | the Parley Authors | `Apache-2.0 OR MIT` |
| `parley_data` | 0.11.1 | 2020 | the Parley Authors | `Apache-2.0 OR MIT` |
| `peniko` | 0.6.1 | 2018 | Raph Levien | `Apache-2.0 OR MIT` |
| `percent-encoding` | 2.3.2 | 2013-2025 | The rust-url developers | `MIT OR Apache-2.0` |
| `phf` | 0.14.0 | 2014-2022 | Steven Fackler | `MIT` |
| `phf_codegen` | 0.14.0 | 2014-2022 | Steven Fackler | `MIT` |
| `phf_generator` | 0.14.0 | 2014-2022 | Steven Fackler | `MIT` |
| `phf_macros` | 0.14.0 | 2014-2022 | Steven Fackler | `MIT` |
| `phf_shared` | 0.14.0 | 2014-2022 | Steven Fackler | `MIT` |
| `pin-project` | 1.1.13 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `pin-project-internal` | 1.1.13 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `pin-project-lite` | 0.2.17 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `pkg-config` | 0.3.34 | 2014 | Alex Crichton | `MIT OR Apache-2.0` |
| `plain` | 0.2.3 | 2017 | jzr | `MIT/Apache-2.0` |
| `png` | 0.18.1 | 2015 | The image-rs Developers | `MIT OR Apache-2.0` |
| `polling` | 3.11.0 | 2025（推定） | Stjepan Glavina 等 2 位贡献者 | `Apache-2.0 OR MIT` |
| `pollster` | 0.4.0 | 2020-2021 | Joshua Barretto | `Apache-2.0/MIT` |
| `polycool` | 0.4.0 | 2018 | Raph Levien | `MIT OR Apache-2.0` |
| `portable-atomic` | 1.15.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `portable-atomic-util` | 0.2.8 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 OR MIT` |
| `precomputed-hash` | 0.1.1 | 2017 | Emilio Cobos Álvarez | `MIT` |
| `presser` | 0.3.1 | 2019 | Embark 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `proc-macro-crate` | 3.5.0 | 2026（推定） | Bastian Köcher | `MIT OR Apache-2.0` |
| `proc-macro2` | 1.0.107 | 2026（推定） | David Tolnay 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `profiling` | 1.0.18 | 2026（推定） | Philip Degarmo | `MIT OR Apache-2.0` |
| `quick-error` | 2.0.1 | 2015 | Paul Colomiets 等 2 位贡献者 | `MIT/Apache-2.0` |
| `quick-xml` | 0.41.0 | 2016 | Johann Tuffe | `MIT` |
| `quinn` | 0.11.12 | 2018 | The quinn Developers | `MIT OR Apache-2.0` |
| `quinn-proto` | 0.11.19 | 2018 | The quinn Developers | `MIT OR Apache-2.0` |
| `quinn-udp` | 0.5.16 | 2018 | The quinn Developers | `MIT OR Apache-2.0` |
| `quote` | 1.0.47 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `r-efi` | 6.0.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` |
| `rand` | 0.10.3 | 2014、2018 | The Rand Project Developers 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `rand_core` | 0.10.1 | 2018-2026 | The Rand Project Developers | `MIT OR Apache-2.0` |
| `rand_pcg` | 0.10.2 | 2014-2017、2018 | The Rand Project Developers | `MIT OR Apache-2.0` |
| `range-alloc` | 0.1.5 | 2023 | the gfx-rs Developers | `MIT OR Apache-2.0` |
| `raw-window-handle` | 0.6.2 | 2019、2020 | Osspial | `MIT OR Apache-2.0 OR Zlib` |
| `raw-window-metal` | 1.1.0 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `read-fonts` | 0.41.0 | 2019 | Fontations Developers | `MIT OR Apache-2.0` |
| `redox_syscall` | 0.9.4 | 2017 | Jeremy Soller | `MIT` |
| `renderdoc-sys` | 1.1.0 | 2022 | Eyal Kalderon | `MIT OR Apache-2.0` |
| `reqwest` | 0.13.5 | 2016-2026 | Sean McArthur | `MIT OR Apache-2.0` |
| `roxmltree` | 0.21.1 | 2018 | Yevhenii Reizner | `MIT OR Apache-2.0` |
| `rustc-hash` | 2.1.3 | 2026（推定） | The Rust Project Developers | `Apache-2.0 OR MIT` |
| `rustc_version` | 0.4.1 | 2016 | The Rust Project Developers | `MIT OR Apache-2.0` |
| `rustix` | 1.1.5 | 2026（推定） | Dan Gohman 等 2 位贡献者 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `rustls` | 0.23.45 | 2016 | Joseph Birr-Pixton <jpixton@gmail.com> | `Apache-2.0 OR ISC OR MIT` |
| `rustls-native-certs` | 0.8.4 | 2016 | Joseph Birr-Pixton <jpixton@gmail.com> | `Apache-2.0 OR ISC OR MIT` |
| `rustls-pki-types` | 1.15.1 | 2023 | Dirkjan Ochtman <dirkjan@ochtman.nl> | `MIT OR Apache-2.0` |
| `rustls-platform-verifier` | 0.7.1 | 2022 | 1Password | `MIT OR Apache-2.0` |
| `rustls-platform-verifier-android` | 0.2.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `rustversion` | 1.0.23 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `same-file` | 1.0.6 | 2017 | Andrew Gallant | `Unlicense/MIT` |
| `schannel` | 0.1.29 | 2015 | Steven Fackler 等 2 位贡献者 | `MIT` |
| `scoped-tls` | 1.0.1 | 2014 | Alex Crichton | `MIT/Apache-2.0` |
| `scopeguard` | 1.2.0 | 2016-2019 | bluss | `MIT OR Apache-2.0` |
| `sctk-adwaita` | 0.10.1 | 2022 | Poly | `MIT` |
| `security-framework` | 3.7.0 | 2015 | Steven Fackler 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `security-framework-sys` | 2.17.0 | 2015 | Steven Fackler 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `semver` | 1.0.28 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `serde` | 1.0.229 | 2026（推定） | Erick Tryzelaar 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `serde_core` | 1.0.229 | 2026（推定） | Erick Tryzelaar 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `serde_derive` | 1.0.229 | 2026（推定） | Erick Tryzelaar 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `servo_arc` | 0.5.0 | 2026（推定） | The Servo Project Developers | `MIT OR Apache-2.0` |
| `shlex` | 2.0.1 | 2015 | comex 等 6 位贡献者 | `MIT OR Apache-2.0` |
| `simd-adler32` | 0.3.10 | 2026（推定） | Marvin Countryman | `MIT` |
| `simd_cesu8` | 1.2.0 | 2026（推定） | Sean C. Roach | `Apache-2.0 OR MIT` |
| `simdutf8` | 0.1.5 | 2024（推定） | Hans Kratz | `MIT OR Apache-2.0` |
| `siphasher` | 1.0.4 | 2012-2016、2016-2026 | Frank Denis | `MIT OR Apache-2.0` |
| `skrifa` | 0.44.0 | 2019 | Fontations Developers | `MIT OR Apache-2.0` |
| `slab` | 0.4.12 | 2019 | Carl Lerche | `MIT` |
| `smallvec` | 1.16.2 | 2018 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `smithay-client-toolkit` | 0.19.2 | 2018 | Elinor Berger 等 3 位贡献者 | `MIT` |
| `smol_str` | 0.2.2 | 2024（推定） | Aleksey Kladov | `MIT OR Apache-2.0` |
| `socket2` | 0.6.5 | 2014 | Alex Crichton 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `stable_deref_trait` | 1.2.1 | 2017 | Robert Grosse | `MIT OR Apache-2.0` |
| `static_assertions` | 1.1.0 | 2017 | Nikolai Vazquez | `MIT OR Apache-2.0` |
| `strict-num` | 0.1.1 | 2022 | Yevhenii Reizner | `MIT` |
| `string_cache` | 0.11.0 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `string_cache_codegen` | 0.11.2 | 2012-2013 | The Servo Project Developers | `MIT OR Apache-2.0` |
| `syn` | 3.0.6 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `synstructure` | 0.14.0 | 2016 | Nika Layzell | `MIT` |
| `system-configuration` | 0.7.0 | 2024 | Mullvad VPN | `MIT OR Apache-2.0` |
| `system-configuration-sys` | 0.6.0 | 2024 | Mullvad VPN | `MIT OR Apache-2.0` |
| `tendril` | 0.5.1 | 2015 | Keegan McAllister 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `termcolor` | 1.4.1 | 2015 | Andrew Gallant | `Unlicense OR MIT` |
| `thiserror` | 2.0.21 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `thiserror-impl` | 2.0.21 | 2026（推定） | David Tolnay | `MIT OR Apache-2.0` |
| `tinyvec` | 1.13.3 | 2019 | Lokathor | `Zlib OR Apache-2.0 OR MIT` |
| `tokio` | 1.53.1 | 2026（推定） | Tokio Contributors | `MIT` |
| `tokio-rustls` | 0.26.6 | 2017 | quininer kel | `MIT OR Apache-2.0` |
| `tokio-util` | 0.7.19 | 2026（推定） | Tokio Contributors | `MIT` |
| `toml_datetime` | 1.1.1+spec-1.1.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `toml_edit` | 0.25.15+spec-1.1.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `toml_parser` | 1.1.3+spec-1.1.0 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `tower` | 0.5.3 | 2019 | Tower Maintainers | `MIT` |
| `tower-http` | 0.6.11 | 2019-2021 | Tower Maintainers | `MIT` |
| `tower-layer` | 0.3.3 | 2019 | Tower Maintainers | `MIT` |
| `tower-service` | 0.3.3 | 2019 | Tower Maintainers | `MIT` |
| `tracing` | 0.1.44 | 2019 | Eliza Weisman 等 2 位贡献者 | `MIT` |
| `tracing-core` | 0.1.36 | 2019 | Tokio Contributors | `MIT` |
| `try-lock` | 0.2.5 | 2016、2018-2023 | Sean McArthur | `MIT` |
| `ttf-parser` | 0.25.1 | 2018 | Caleb Maclennan 等 4 位贡献者 | `MIT OR Apache-2.0` |
| `unicode-segmentation` | 1.13.3 | 2015 | kwantam 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `unicode-width` | 0.2.2 | 2015 | kwantam 等 2 位贡献者 | `MIT OR Apache-2.0` |
| `url` | 2.5.8 | 2013-2025 | The rust-url developers | `MIT OR Apache-2.0` |
| `utf8_iter` | 1.0.4 | 2023（推定） | Henri Sivonen | `Apache-2.0 OR MIT` |
| `vello_common` | 0.2.0 | 2020 | the Vello Authors | `Apache-2.0 OR MIT` |
| `vello_cpu` | 0.2.0 | 2020 | the Vello Authors | `Apache-2.0 OR MIT` |
| `version_check` | 0.9.5 | 2017-2018 | Sergio Benitez | `MIT/Apache-2.0` |
| `walkdir` | 2.5.0 | 2015 | Andrew Gallant | `Unlicense/MIT` |
| `want` | 0.3.1 | 2018-2019 | Sean McArthur | `MIT` |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | 2025（推定） | The Cranelift Project Developers | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `wasip2` | 1.0.4+wasi-0.2.12 | 2026（推定） | 见该 crate 仓库的版权声明 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `wasm-bindgen` | 0.2.129 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `wasm-bindgen-futures` | 0.4.79 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `wasm-bindgen-macro` | 0.2.129 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `wasm-bindgen-macro-support` | 0.2.129 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `wasm-bindgen-shared` | 0.2.129 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `wayland-backend` | 0.3.17 | 2015 | Elinor Berger | `MIT` |
| `wayland-client` | 0.31.15 | 2015 | Elinor Berger | `MIT` |
| `wayland-csd-frame` | 0.3.0 | 2023 | Kirill Chibisov | `MIT` |
| `wayland-cursor` | 0.31.14 | 2015 | Elinor Berger | `MIT` |
| `wayland-protocols` | 0.32.13 | 2015 | Elinor Berger | `MIT` |
| `wayland-protocols-plasma` | 0.3.12 | 2015 | Elinor Berger | `MIT` |
| `wayland-protocols-wlr` | 0.3.12 | 2015 | Elinor Berger | `MIT` |
| `wayland-scanner` | 0.31.11 | 2015 | Elinor Berger | `MIT` |
| `wayland-sys` | 0.31.11 | 2015 | Elinor Berger | `MIT` |
| `web-sys` | 0.3.106 | 2014 | The wasm-bindgen Developers | `MIT OR Apache-2.0` |
| `web-time` | 1.1.0 | 2023 | dAxpeDDa | `MIT OR Apache-2.0` |
| `web_atoms` | 0.3.0 | 2014 | The html5ever Project Developers | `MIT OR Apache-2.0` |
| `weezl` | 0.1.12 | 2025（推定） | The image-rs Developers | `MIT OR Apache-2.0` |
| `wgpu` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-core` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-core-deps-apple` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-core-deps-emscripten` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-core-deps-windows-linux-android` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-hal` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-naga-bridge` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `wgpu-types` | 29.0.4 | 2025 | gfx-rs developers | `MIT OR Apache-2.0` |
| `winapi-util` | 0.1.11 | 2017 | Andrew Gallant | `Unlicense OR MIT` |
| `windows` | 0.62.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-collections` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-core` | 0.62.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-future` | 0.3.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-implement` | 0.60.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-interface` | 0.59.3 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-link` | 0.2.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-numerics` | 0.3.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-registry` | 0.6.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-result` | 0.4.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-strings` | 0.5.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-sys` | 0.61.2 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows-targets` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows-threading` | 0.2.1 | 2025（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0` |
| `windows_aarch64_gnullvm` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_aarch64_msvc` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_i686_gnu` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_i686_gnullvm` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_i686_msvc` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_x86_64_gnu` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_x86_64_gnullvm` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `windows_x86_64_msvc` | 0.52.6 | 2024（推定） | Microsoft | `MIT OR Apache-2.0` |
| `winnow` | 1.0.4 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT` |
| `wit-bindgen` | 0.57.1 | 2026（推定） | Alex Crichton | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `x11-dl` | 2.21.0 | 2023（推定） | daggerbot 等 3 位贡献者 | `MIT` |
| `x11rb` | 0.13.2 | 2019 | Uli Schlachter 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `x11rb-protocol` | 0.13.2 | 2019 | Uli Schlachter 等 3 位贡献者 | `MIT OR Apache-2.0` |
| `xcursor` | 0.3.11 | 2020 | Samuele Esposito | `MIT` |
| `xkbcommon-dl` | 0.4.2 | 2023 | Francesca Frangipane | `MIT` |
| `xkeysym` | 0.2.1 | 2022-2023 | John Nunley | `MIT OR Apache-2.0 OR Zlib` |
| `xml-rs` | 0.8.29 | 2014 | Vladimir Matveev | `MIT` |
| `yeslogic-fontconfig-sys` | 6.0.1 | 2013、2014、2016、2019 | Austin Bonander 等 3 位贡献者 | `MIT` |
| `zerocopy` | 0.8.59 | 2019、2023 | The Fuchsia Authors | `BSD-2-Clause OR Apache-2.0 OR MIT` |
| `zerocopy-derive` | 0.8.59 | 2019、2023 | The Fuchsia Authors | `BSD-2-Clause OR Apache-2.0 OR MIT` |
| `zeroize` | 1.9.0 | 2018-2026 | The RustCrypto Project Developers | `Apache-2.0 OR MIT` |
| `zune-core` | 0.5.3 | 2026（推定） | 见该 crate 仓库的版权声明 | `MIT OR Apache-2.0 OR Zlib` |
| `zune-jpeg` | 0.5.15 | 2026（推定） | caleb | `MIT OR Apache-2.0 OR Zlib` |

## Apache License 2.0（`Apache-2.0`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `ab_glyph` | 0.2.32 | 2025（推定） | Alex Butler | `Apache-2.0` |
| `ab_glyph_rasterizer` | 0.1.10 | 2025（推定） | Alex Butler | `Apache-2.0` |
| `codespan-reporting` | 0.13.1 | 2025（推定） | Brendan Zabarauskas | `Apache-2.0` |
| `dunce` | 1.0.5 | 2024（推定） | Kornel | `CC0-1.0 OR MIT-0 OR Apache-2.0` |
| `gethostname` | 1.1.0 | 2025（推定） | Sebastian Wiesner | `Apache-2.0` |
| `gl_generator` | 0.14.0 | 2019（推定） | Brendan Zabarauskas 等 3 位贡献者 | `Apache-2.0` |
| `glutin_wgl_sys` | 0.6.1 | 2025（推定） | Kirill Chibisov | `Apache-2.0` |
| `khronos_api` | 3.1.0 | 2019（推定） | Brendan Zabarauskas 等 4 位贡献者 | `Apache-2.0` |
| `moxcms` | 0.8.1 | 2026（推定） | Radzivon Bartoshyk | `BSD-3-Clause OR Apache-2.0` |
| `owned_ttf_parser` | 0.25.1 | 2025（推定） | Alex Butler | `Apache-2.0` |
| `pxfm` | 0.1.30 | 2026（推定） | Radzivon Bartoshyk | `BSD-3-Clause OR Apache-2.0` |
| `ring` | 0.17.14 | 2015-2025 | Brian Smith | `Apache-2.0 AND ISC` |
| `spirv` | 0.4.0+sdk-1.4.341.0 | 2026（推定） | Lei Zhang | `Apache-2.0` |
| `sync_wrapper` | 1.0.2 | 2024（推定） | Actyx AG | `Apache-2.0` |
| `winit` | 0.30.13 | 2026（推定） | The winit contributors 等 2 位贡献者 | `Apache-2.0` |

## BSD 2-Clause Simplified（`BSD-2-Clause`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `arrayref` | 0.3.9 | 2015 | David Roundy | `BSD-2-Clause` |

## BSD 3-Clause New/Revised（`BSD-3-Clause`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `encoding_rs` | 0.8.42 | 2026（推定） | Henri Sivonen | `(Apache-2.0 OR MIT) AND BSD-3-Clause` |
| `subtle` | 2.6.1 | 2016-2017、2016-2024 | Isis Lovecruft 等 2 位贡献者 | `BSD-3-Clause` |
| `tiny-skia` | 0.11.4 | 2011、2020 | Yevhenii Reizner | `BSD-3-Clause` |
| `tiny-skia-path` | 0.11.4 | 2011、2020 | Yevhenii Reizner | `BSD-3-Clause` |

## ISC License（`ISC`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `aws-lc-rs` | 1.18.1 | 2026（推定） | AWS-LibCrypto | `ISC AND (Apache-2.0 OR ISC)` |
| `libloading` | 0.8.9 | 2015 | Simonas Kazlauskas | `ISC` |
| `rustls-webpki` | 0.103.15 | 2015 | Brian Smith | `ISC` |
| `untrusted` | 0.9.0 | 2015-2016 | Brian Smith | `ISC` |

## zlib License（`Zlib`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `foldhash` | 0.2.0 | 2024 | Orson Peters | `Zlib` |
| `slotmap` | 1.1.1 | 2021 | Orson Peters | `Zlib` |
| `zlib-rs` | 0.6.8 | 2026（推定） | 见该 crate 仓库的版权声明 | `Zlib` |

## Mozilla Public License 2.0（`MPL-2.0`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `cssparser` | 0.38.0 | 2026（推定） | Simon Sapin | `MPL-2.0` |
| `cssparser-macros` | 0.7.1 | 2026（推定） | Simon Sapin | `MPL-2.0` |
| `dtoa-short` | 0.3.5 | 2024（推定） | Xidorn Quan | `MPL-2.0` |
| `selectors` | 0.41.0 | 2026（推定） | The Servo Project Developers | `MPL-2.0` |

## Unicode License v3（`Unicode-3.0`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `icu_collections` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_locale_core` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_locale_fallback` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_locale_fallback_data` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_normalizer` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_normalizer_data` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_properties` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_properties_data` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_provider` | 2.3.1 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_segmenter` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `icu_segmenter_data` | 2.3.0 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `litemap` | 0.8.3 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `potential_utf` | 0.1.6 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `tinystr` | 0.8.4 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `unicode-ident` | 1.0.26 | 1991-2023 | David Tolnay | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `writeable` | 0.6.4 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `yoke` | 0.8.3 | 2020-2024 | Manish Goregaokar | `Unicode-3.0` |
| `yoke-derive` | 0.8.4 | 2020-2024 | Manish Goregaokar | `Unicode-3.0` |
| `zerofrom` | 0.1.8 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `zerofrom-derive` | 0.1.8 | 2020-2024 | Manish Goregaokar | `Unicode-3.0` |
| `zerotrie` | 0.2.5 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `zerovec` | 0.11.8 | 2020-2024 | The ICU4X Project Developers | `Unicode-3.0` |
| `zerovec-derive` | 0.11.6 | 2020-2024 | Manish Goregaokar | `Unicode-3.0` |

## Creative Commons Zero v1.0 Universal（`CC0-1.0`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `hexf-parse` | 0.2.1 | 2021（推定） | Kang Seonghoon | `CC0-1.0` |

## Community Data License Agreement Permissive 2.0（`CDLA-Permissive-2.0`）

| crate | 版本 | 版权年份 | 版权人 | SPDX 声明 |
| --- | --- | --- | --- | --- |
| `webpki-root-certs` | 1.0.9 | 2026（推定） | 见该 crate 仓库的版权声明 | `CDLA-Permissive-2.0` |

## 许可政策与门禁

本项目遵循 [AGENTS.md 的许可证政策](../AGENTS.md)：白名单为 MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL / Zlib / CC0-1.0 / Unicode-3.0 / CDLA-Permissive-2.0，**禁 GPL / AGPL**。其中 Zlib、CC0-1.0、Unicode-3.0、CDLA-Permissive-2.0 是已选 crate 栈（wgpu / icu4x / reqwest）无法回避的宽松非 copyleft 传递依赖，逐条理由见 [`deny.toml`](../deny.toml)。

门禁由 `cargo deny check` 强制（**含传递依赖**，直接依赖合规不代表传递依赖合规）。本名单是机器生成的**阅读视图**，判定以`cargo deny check` 与 [`docs/dependencies.html`](dependencies.html)（含许可证全文）为准。

## 本文件如何生成

依赖变动后按 AGENTS.md 要求重新生成：

```bash
# 1) 从 Cargo.lock + 本地 registry 源码包提取 name/version/authors/LICENSE 年份
# 2) 缺年份的 crate 从 crates.io index 缓存取 pubtime 补齐
# 3) 读各 crate 的 SPDX 声明，按标识集合分组渲染本文件
cargo about generate about.hbs -o docs/dependencies.html
```
