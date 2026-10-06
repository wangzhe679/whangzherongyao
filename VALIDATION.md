# anti-4.8-1 本地验证记录

日期：2026-10-06。来源：同级 `anti-4.8`，官方基线 v4.8.4-beta.1。平台：Windows x64。所有改动与检查均针对副本，原目录未修改。

| 检查 | 结果 |
|---|---|
| `npm run build` | 通过；TypeScript 与 Vite 生产构建完成；现有包体积提示保留 |
| `cargo fmt -- --check` | 通过 |
| `cargo clippy --offline --all-targets --all-features` | 通过；保留现有编译器／Clippy 告警，未使用自动修复扩大范围 |
| `cargo test --lib strict_` | 最终39通过、0失败，含文本期限、旧锁迁移、重复邮箱和OpenCode重试 |
| `cargo test --lib proxy::pipeline` | 最终32通过、0失败，包含8项思考策略测试 |
| `cargo test --lib proxy::common::variant_mapping` | 22 通过、0 失败 |
| `cargo test --lib memory_reclamation_` | 最终15通过、0失败，含可控ingest并发交错及SQL失败路径 |
| `cargo test --lib proxy::runtime_limits::tests` | 3 通过、0 失败 |
| `cargo test --lib proxy::handlers::common::tests` | 最终3通过、0失败 |
| `cargo test --lib claude_non_stream_capture` | 1通过、0失败；备用JSON工具捕获的因果锚点 |
| `git diff --check` | 通过 |
| 浏览器预览 | 合成账号数据验收通过，未连接真实账号和上游 |

最终修复包括文本期限、预算权限、ingest旧快照覆盖、prune完整性检查、重复邮箱账本保护和OpenCode的429/503重试截断。最后一轮串行执行上表strict、pipeline、memory、Claude专项和common，共90次用例执行，因筛选重叠为88个不同用例；与前轮未变模块结果合计112个不同通过用例。没有运行整个仓库的全量测试，也不能据此保证所有生产路径没有bug。

发布版本同步后重新执行前端生产构建、格式和全目标全特性Clippy，均通过；依赖版本未升级。浏览器合成验收沿用前轮结果。原有账号页新锁刷新、身份与真实上游验收边界见 `AUDIT_33.md`；修复及旧脚本只读检查见 `REVIEW_UPDATE_SAFETY.md`。

## 模型锁

- 七个模型的支持数、可用数、假锁与短锁累计次数分别记录。
- 同账号 Claude 两模型共享已确认的真 429 截止时间；Gemini 三个 Flash tiered 与 Pro low 共享，Lite 独立。
- 普通 429、503、无有效上游截止时间及模型容量错误不会扩散为共享真锁。
- 普通 Opus、思考前缀 Opus 和真实 Thinking 模型名使用同一锁身份；旧 Flash 别名兼容新 Tiered 身份。
- 保留既有 10／30 分钟短锁规则、并发去重、不提前解锁、计数跨到期和重启保留、同邮箱重新导入保护。
- 旧账号快照不会把假锁升级成真锁；旧 Claude 不明来源锁保守保留到期。
- 持久化文件保留旧版本有效锁投影，新版独立计数不受旧投影污染。
- 仪表盘摘要与原详情接口统计一致，摘要不读取账号详情文件或复制完整锁正文。

## Claude 思考

- 普通 `claude-opus-4-6` 映射至 `claude-opus-4-6-thinking`，客户端输出不含思考内容。
- `[思考]claude-opus-4-6` 映射至相同真实模型，开启思考输出，默认正数预算 1024；不使用 -1。
- Gateway保留UI正数自定义预算，Client保留客户端正数预算，均在原模型能力上限内；缺省或非正数回退1024。名称决定可见性。
- OpenAI Chat、Responses、Claude 与 Gemini 四协议参数均纳入测试；流式分片、非流式过滤和后续 Responses 会话模型继承纳入检查。
- 隐藏客户端思考内容仍保留内部缓存和工具签名；可见请求保留上游实际返回的思考片段。
- 动态模型 ID 改为正常拥有生命周期的字符串，原有模型映射回归通过。

## 内存回收

- 回收可从 SQLite 恢复的闲置缓存，当前请求的 Arc 快照继续有效，完整历史可以回读。
- 聚合缓存预算与会话裁剪不会删除完整 SQLite 历史；写入失败及无持久化副本的数据保留。
- 并发追加、升级、清理和重新创建会话不会被旧加载快照覆盖。
- 工具签名可从 SQLite 回读；SQL 忽略的占位签名保留；原始签名自愈编码与回读保持兼容。
- 定时维护随监控实例释放而停止，过期缓存按原生命周期清理。
- 工具日志分组不再泄漏字符串，输出内容保持一致。
- 完整流式流量日志保留，减少重复字符串分配；不通过截断日志或强制清空有效签名压低内存。

## 界面验收

使用仅允许读取的本地合成数据服务预览构建产物：

- 导航和路由移除独立“模型冷却”页面。
- 仪表盘顶部保留账号池、403、内存和在途请求信息；紧凑表格显示七个模型。
- 运行设置、403 维护、账号健康额度汇总、当前和推荐账号仍可展开访问。
- 账号管理中的七个模型额度和活动冷却倒计时保留；合成 Claude 真锁账号的两个模型显示同一截止时间。
- 本轮真429／短锁、最早与最晚到期、6d至1d分布预览通过；已加载冷却与全部锁明确区分。

## 本机工具链与日志

使用工作区便携 MSVC／SDK／Rust 工具链；没有安装或升级项目依赖。编译 OAuth 值为测试占位值，测试数据目录隔离在 `tmp/anti-4.8-1-test-data`，没有使用生产账户。

测试在离线模式下运行，追加 `--config "profile.dev.package.boring-sys2.opt-level=1"` 适配本机原生依赖 Debug CRT；测试启动器嵌入 Common Controls v6 清单。共享测试 EXE 清单写入曾因并行启动竞争失败，最终串行运行已通过；该适配不改变断言。

最终日志位于工作区 `tmp/`：

- `anti-4.8-1-frontend.log`
- `anti-4.8-1-fmt-final.log`
- `anti-4.8-1-clippy-final.log`
- `anti-4.8-1-{strict,pipeline,mapping,memory,runtime,retry}-tests-final.log`

本轮只读统计新增后的检查日志为 `anti-4.8-1-stats-{frontend,fmt,clippy,tests}.log`。前端构建、格式、Clippy、30项锁测试与合成浏览器预览通过；保留现有编译告警。

送GitHub前的最终日志：`anti-4.8-1-github-{strict,pipeline,memory,claude,retry,fmt,clippy,frontend}.log`。所有测试数据位于工作区临时目录，未使用实际账号或精确锁文件。

## 验证边界

未使用真实生产账号请求上游，未做生产长时间内存／并发压测，未部署。按用户最新指定的main＋tag流程发布v4.8.4-beta.1-fixed.4，使用原release workflow执行Linux编译、Docker运行库/health/Web UI/docker-load校验、构建产物上传与tar.gz预发布。更新远端main前保存原CPA提交f364d317bf8031fac9b888b246c78405a6c404dd的备份引用。远端构建结果以对应Actions运行记录为准。

缓存256MiB是L1回收目标，不是整个进程RSS的硬上限；活动长响应、高并发以及不能安全持久化的数据仍可能产生内存峰值。完整行为说明见 `CUSTOM_4.8-1.md`，副本中的 `CUSTOM_MIGRATION.md` 仅为历史记录。
