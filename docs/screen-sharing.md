# 屏幕共享配置与验收

目标：桌面或程序窗口共享；可调码率、分辨率、帧率；可开关共享声音；服务器可限制。现有语音优先，不改变语音编码、抖动缓冲和路由策略。

## v0.8.5：独立固定 UDP 端口

服务端的所有屏幕共享 WebRTC 连接复用一个固定 UDP 端口，默认 `7010`；原有语音 UDP 端口保持独立。多个房间的会话、身份验证和加密连接仍分别管理，共用端口不会让不同房间互相收到画面。每个房间一位分享者，其他成员主动加入观看，不同房间可在服务器容量限制内同时共享。

编辑 TOML 中已有的 `[screen_share]` 段，下面只列出网络相关字段：

```toml
[screen_share]
enabled = true
# 公网云服务器填写自己的公网 IP；以下是文档示例地址。
public_ip = "203.0.113.10"
udp_port = 7010
```

- 云安全组、系统防火墙及 NAT 转发放行这个 UDP 端口。Docker 使用对应映射，例如 `-p 7010:7010/udp`；外部端口与配置端口保持一致。
- `public_ip` 是向客户端通告的地址，云服务器使用公网地址，不能填其仅在云内可达的私网地址。可直连的局域网部署可保留空值，由 ICE 收集本机地址。
- `udp_port` 不能与 `voice_addr` 使用同一个本地 UDP 端口，也应避开其他程序占用的端口。修改 `public_ip` 或 `udp_port` 后重启服务端；端口占用时根据启动错误检查配置。
- `max_bitrate_kbps`、`max_width`、`max_height`、`max_fps`、`allow_audio`、`max_viewers`、`max_rooms` 的含义不变。后台保存的共享策略仍优先于 TOML 中的画质与容量设置，公网地址和端口仍只从 TOML 读取。
- 后台关闭或改变共享策略会结束已有共享；保存相同策略不会打断。视频与共享声音不进入语音队列，但仍会消耗同一台服务器的带宽，请按服务器带宽设置码率和观看容量。

### 从旧端口范围升级

推荐把原 `udp_port_min` / `udp_port_max` 替换为 `udp_port = 7010`（或原范围起始端口）。为兼容已有配置，未填写 `udp_port` 时使用原 `udp_port_min` 作为唯一端口；旧的上限字段不再分配一段端口，新字段存在时优先使用新字段。

先确保固定端口可达，更新服务端并重启，检查共享正常后，再删除旧范围其余端口的防火墙规则。更新二进制时保留原数据目录、身份与后台策略文件。服务端重启会短暂断开现有通话和共享，客户端重连后需要重新发起共享。

**此端口变更只需更新服务端。** 已经支持屏幕共享的客户端通过 SDP / ICE 协商获取新端口，无需硬编码或配置端口范围。没有屏幕共享功能的旧客户端继续使用原语音功能，不会仅因服务端升级而获得共享界面。客户端 / 服务端的当前源码版本均为 `0.8.5`，安装包是否已发布请以实际下载文件为准。

### 使用与验证边界

在当前语音房间选择屏幕或窗口并确认共享；其他成员点击观看后进入嵌入式屏幕标签页，可全屏。切回房间或聊天保留共享连接，明确停止观看、离房或断线才释放。共享声音默认关闭，用户可在允许的范围内调整画质、帧率与码率。

macOS 使用系统选择器选择窗口或整个屏幕，原生系统声音要求 macOS 14.2+。Windows 系统共享声音可能包含客户端通话回放；原生选择器和跨设备表现仍需实际设备验证。本机合成媒体、浏览器与代码测试不代表所有平台原生采集或公网听感通过。

下文保留开发过程及其当时的验收结果；其中“本次未部署”“尚未完成”等描述均属于对应历史阶段，不表示 v0.8.5 的当前部署状态。最新运行与发布状态以当前版本发布记录为准。

## 最初设计约束与历史验收记录（2026-09-24）

- 同一语音房间一位分享者，成员主动观看。离房、掉线、分享源关闭时释放采集与传输。
- 使用独立 WebRTC/SRTP 媒体连接，信令沿用已认证 TLS 控制连接。视频不进入现有语音队列。
- 服务端 SFU 只转发，不解码/转码。观看者采用独立有界发送队列，慢观看者不阻塞其他观看者和语音。
- 桌面/窗口由系统选择器显式授权。音频必须显式开启；系统未提供音轨时如实提示，不能显示为共享成功。
- 用户参数与服务端限制取交集，服务端验证房间成员身份、共享会话标识、码率及媒体格式。离房后不得继续观看。
- 默认 1280×720 / 30 fps / 2000 kbps，音频关闭。服务端支持禁用共享及配置最高分辨率、帧率、码率、观看人数和并发房间数。
- 旧客户端继续通话。此功能需要新服务端和新客户端；本次不自动部署、不变更发布版本。

## 验收清单（未完成项保持明确）

- [ ] 服务端配置、校验与权限边界
- [ ] 独立 SFU 视频/共享音频转发，慢观看者隔离、丢包关键帧恢复
- [ ] 控制协议、客户端桥接、生命周期与房间联动
- [ ] 桌面/窗口采集、声音开关与停止释放
- [ ] 用户码率/分辨率/帧率调节，服务器上限提示与执行
- [ ] 管理端配置及运行状态
- [ ] 明暗主题、多人房间布局与可体验预览
- [ ] 真实端到端视频及音轨测试、语音并发回归
- [ ] Windows 客户端能力验证；其他平台能力明确检测与验证

## 当前验收结论（2026-09-24，后续记录更新于本表）

| 要求 | 已有证据 | 尚待确认 |
| --- | --- | --- |
| 桌面与程序窗口选择 | 客户端提供两个入口；真实组件交互通过；WKWebView API 可用 | 用户已确认 R3 窗口与整个屏幕均可见；正式客户端端到端与 Windows WebView2 实机验收仍待完成 |
| 分辨率、帧率、码率自行调节 | 真实组件设置与边界测试；浏览器 360p、720p、1080p 实际编解码通过 | 原生捕获源约束和长时间表现；目前参数在开始共享前选择 |
| 服务器限制 | TOML、管理页面、策略持久化与权限测试；媒体守卫测试 | 公网部署环境的端口、网络和负载验收 |
| 共享声音开关 | 合成音轨经实际 WebRTC 转发、解码，静音/恢复能量验证通过 | 真实系统/窗口是否提供音轨，是否混入本客户端语音回放；不支持时提示仅共享画面 |
| 停止与离房清理 | 控制器、Manager 生命周期测试；浏览器结束时轨道与观看连接释放 | 真实客户端切房/断线后系统采集指示消失 |
| 保持现有语音 | 全量 Go、Rust 客户端测试通过；本机五人合成 UDP 语音与视频并发通过 | 真实设备听感与公网带宽竞争，不能用本机测试代替 |

完整功能尚未验收通过；无生产部署、无发布版本变更。原生验证不能由更多模拟测试替代。上面的原始清单保留为最终交付检查项，下面为逐次测试记录。

## 参考

- WebView2 ScreenCaptureStarting: https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2.screencapturestarting
- Pion SFU 示例: https://github.com/pion/example-webrtc-applications/tree/main/sfu-ws
- Pion API: https://pkg.go.dev/github.com/pion/webrtc/v4

## 当前验证记录（2026-09-24）

- `npm --prefix desktop run build` 通过。控制器文件改为 `screenShareController.ts`，避免大小写不敏感文件系统的模块冲突。
- 原生 `cargo test ... screenshare --lib` 通过标识符精度测试；这不代表采集能力已验证。
- `npm --prefix desktop run test:screen-share`：4 项通过，覆盖参数上限、跨连接消息隔离、取消观看协商、采集授权晚返回时释放轨道。
- Go 共享/config/control/main 包 `go test -race` 通过（包含已有缓存结果）。共享测试使用本机 WebRTC 连接及合成 RTP，不等同于实际画面解码、声音听感或 Windows 系统采集验收。
- 收起画面保持播放器挂载，保留音轨播放；取消协商使用取消本人连接命令，避免请求结束其他成员的共享。
- 未完成：真实画面/音频端到端测试、目标平台采集验证、管理页面、拥塞与语音并发验证。未部署、未提交。

### 浏览器编解码验收进展

新增 `server/internal/screenshare/browser_test.go` 和 `desktop/scripts/screen-share-browser.test.mjs`，通过实际客户端控制器、Chrome 编码/解码、服务端 Manager 联调，检查动态画面像素、共享音轨能量及开关、停止释放。测试以 canvas/合成音轨作为输入，不代表操作系统桌面/窗口采集已验收。

首次测试暴露服务端将 Chrome 的纯填充 RTP 带宽探测包误判为损坏 VP8 并终止共享。已修复：填充包计入限速，不计作视频帧、不转发；新增回归覆盖合法填充、空非法包及填充超限。浏览器测试就绪等待改为显式轮询信令（原 waitForFunction 内的异步信令轮询未持续刷新）。修复后已通过真实 640×360 动态画面像素检查、音轨开关前后能量检查、停止采集与观看释放检查。此结果不覆盖原生 WebView 或系统桌面/窗口采集。测试通过设置 `EDEN_SCREEN_BROWSER=1` 启用，另需 `PLAYWRIGHT_MODULE`、`CHROME_EXECUTABLE`；普通 Go 测试会显式跳过。

### 状态同步回归

- 服务端状态快照附带递增 revision，Rust 桥接以字符串保留 64 位精度；客户端忽略乱序旧快照，RPC 仍正常完成。
- 状态应用统一进入 receive，避免 refresh/publish/subscribe 完成时再次覆盖新广播；协商完成后确认共享标识仍匹配再显示画面。
- 6 项控制器测试通过，含大整数序号、结束后迟到的发布回复、晚返回的状态查询；原生标识符/序号精度测试、前端构建及相关 Go 包竞态测试通过。

### macOS 原生 WebKit 能力检查

在 macOS 26.4.1 执行 `desktop/scripts/screen-share-webkit-probe.swift`，使用真实 WKWebView，匹配 Wry 的媒体权限委托及自动播放设置。结果：安全上下文为 true，getDisplayMedia 和 RTCPeerConnection 均为 function，视频编码列表包含 VP8。脚本不采集屏幕、窗口或麦克风。

这是 API/编码器存在性的验证，尚不证明实际系统选择器、桌面/窗口持续采集和系统声音可用；也不能外推到最低支持的 macOS 12 或 Windows/Linux。WebKit 曾存在与 WKUIDelegate 媒体权限委托相关的 getDisplayMedia 回归，应保留实际版本验证：https://bugs.webkit.org/show_bug.cgi?id=274896 。

### 服务端策略生命周期验证

新增真实 PeerConnection 协商回归：禁用共享关闭已存在发布端并拒绝新发布；非法策略不影响现有共享；取消的发布请求不遗留房间；重复保存相同策略不关闭连接。修复 SetPolicy 对相同配置也会结束共享的问题。共享包 `go test -race -count=1` 通过。本组测试验证 Manager 行为，不代表尚未接入的管理端页面已完成。

### 后台共享限制

管理端新增“屏幕共享”页面，可设置启用、声音权限、分辨率、帧率、码率、观看人数、同时共享房间数。沿用已有账号身份、只读权限、CSRF 防护及审计。策略使用 revision 防止管理员相互覆盖，先原子保存 `security.data_dir/screen-sharing.json`，再应用运行时限制；磁盘保存失败不改变当前运行策略。此文件优先于 TOML 画质设置，公网 IP 和端口仍来自 TOML。修改策略会结束现有共享；不改变配置的保存不打断共享。页面明确提示生效影响。

GET/POST `/api/screen-sharing` 已接入；修改后刷新订阅状态的所有房间，空闲房间也能收到启用状态。页面尚待视觉和交互验收。

后台接口验证已通过：webadmin/screenshare/control/main 四个包竞态测试；新增权限、CSRF、只读、冲突、持久化及无效参数回归；补跑配置写入失败测试通过，运行策略和 revision 保持原值。后台 JavaScript 语法检查通过，实际页面布局和操作仍需浏览器验收。

### 管理页面浏览器验收

隔离 admin-demo 已接入共享配置。`desktop/scripts/screen-share-admin.test.mjs` 使用真实浏览器登录本机演示，验证保存 1800 kbps / 20 fps、刷新后读取、范围校验、无脚本错误及 820×560 弹窗。修复紧凑窗口下保存按钮被裁切的问题：设置内容独立滚动，底部操作始终可见。截图 `/tmp/eden-screen-admin-page.png` 和 `/tmp/eden-screen-admin-dialog.png` 已人工查看。测试后停止了演示进程，未操作生产服务器。

### 原生采集验收入口与当前限制

`screen-share-webkit-probe.swift --capture` 现可打开本机预览窗口，通过按钮在系统选择器选择桌面/窗口，请求共享音轨，显示实际轨道参数并提供停止按钮；默认模式仍只检查 API，不采集。测试不上传画面。已编译启动该模式，但电脑控制工具报告 Mac 已锁定、无法自动解锁，因而没有执行系统选择器操作，实际采集仍未验收。已请求用户方便时手动解锁；不修改锁屏策略。

客户端补充采集取消、权限不足、画质约束不支持等提示，并规范化画质输入为有效范围内的有限整数。8 项客户端测试及前端构建通过。

### 语音与视频并发传输检查

真实浏览器编解码测试同时运行同进程的实际 UDP voice.Relay：5 个会话、每 20 ms 每人一包、120 字节合成负载，各接收其他 4 人，检查序号连续性和投递数量。一次本机竞态检测运行发送 1140 包，应转发/实际收到均为 4560 包，最大观测本机转发延迟 1.433 ms；视频像素、共享音轨开关和释放测试同时通过。此用例采用旧格式的合成语音包，不覆盖加密通话、Opus 解码、音频设备、公网带宽竞争或主观听感，不能用于承诺部署后不卡顿。

已停止无法操作系统选择器的本机 Swift 采集测试进程。Mac 解锁后仍需重新执行原生采集验收。

### 默认画质与重传观测

浏览器集成测试增加 `EDEN_SCREEN_QUALITY` 参数，可复用同一真实编解码链路验证不同配置；输出接收端 framesDecoded / packetsLost / nackCount / pliCount。以默认 1280×720、30 fps、2000 kbps 配置完成一次并发测试：成功解码 1280×720 画面，接收端观测 19 帧、丢包 0、NACK 0、PLI 2；音轨开关/清理通过。同期五人语音发送 850 包、应转发及实际接收均为 3400 包，最大本机转发延迟 9.384 ms。该短测不能证明持续 30 fps、公网表现或用户实际桌面采集成功。

### 全量服务端回归与原生验收阻碍

服务端 `go test ./...` 全部通过（包含原有语音、安全、观影、聊天、管理等包；默认跳过需要显式开启的浏览器测试，其独立结果见上文）。

再次检查时 Mac 已不再报告锁定，但调用电脑控制工具打开原生测试 App 返回 `Computer Use permissions are not granted`。已请求用户开启电脑控制权限或自行操作选择器；没有绕过权限。原生采集测试脚本已编译为 `/tmp/EdenScreenCaptureProbe.app`，仍未完成系统选择器验收，不能据 API 存在或编译成功视为采集通过。

用户选择自行验证桌面/窗口共享。手动步骤：打开 `/tmp/EdenScreenCaptureProbe.app`；点击“选择窗口”，在系统选择器选一个普通测试窗口，确认本机画面随窗口变化；停止后点击“选择桌面”重复；记录页面 tracks 中是否包含 kind=audio，以及错误名称/消息。停止采集后系统共享指示应消失。该工具只测试原生采集，不能替代完整客户端的发布/观看验证。

### 客户端组件交互回归（2026-09-24）

- 完整 Rust 客户端测试在允许本机 socket 后通过：290 passed、2 ignored；此前沙箱拒绝 socket 导致的失败已复验。
- 新增 `desktop/scripts/screen-share-ui.test.mjs`：渲染真实 ScreenShare 组件和全量客户端 CSS，仅模拟原生 IPC 与媒体控制器状态。覆盖服务器参数限制呈现、窗口来源选择、声音开关、收起/展开，以及管理员观看时结束共享；不代替原生采集或真实媒体链路测试。
- 修复管理员观看中缺少“结束共享”入口；处理手动播放失败的 Promise，保留重试按钮。
- 组件测试和前端构建通过，检查设置面板截图 `/tmp/eden-screen-client-settings.png`。
- 用户选择手动验证桌面/窗口采集；仍待反馈，未据此认定原生采集验收通过。

### 系统停止采集的竞态清理

系统可能在 applyConstraints 尚未完成时结束采集。现在拿到视频轨道后立即注册结束事件，并在约束应用前后确认轨道仍存活，避免已停止的采集继续协商，同时及时释放共享声音轨道。新增两项回归：约束等待中系统停止、采集返回时轨道已结束。控制器测试 10 项通过，前端生产构建通过；这些是生命周期测试，不替代系统选择器人工验收。

### 1080p 编解码补充验证

以 1920×1080 / 30 fps / 4000 kbps 请求运行浏览器与实际 SFU 联调：接收解码画面为 1920×1080，观测 17 帧、丢包 0、NACK 0、PLI 2；音轨开关及释放通过。同期五人合成语音发送 1070 包，应转发/收到均为 4280，最大本机转发延迟 888 微秒。日志 `/tmp/eden-screen-1080.log`。这是约 4 秒的功能测试，不证明持续帧率、原生采集或公网性能。

### 用户实测反馈：测试窗口 mediaDevices 缺失

用户点击采集得到 TypeError（navigator.mediaDevices 未定义）。旧测试使用 loadHTMLString 配合 localhost baseURL，不能据此推断 App 包里的文档实际具有可信来源。测试入口改为只绑定 127.0.0.1 的临时 HTTP 服务，加载真实回环 URL，并在页面展示 secure/origin/mediaDevices/display 诊断信息；接口缺失时直接报告能力，而不是触发 TypeError。新增 --check 允许以同一 App 包身份检查能力，不进行采集。

重新编译并更新 `/tmp/EdenScreenCaptureProbe.app`，以包内可执行文件 --check 验证：secure=true、origin=http://127.0.0.1:<临时端口>、mediaDevices=object、display=function。仅证明修正后的测试入口 API 可见，实际桌面/窗口采集和正式打包客户端自定义协议来源仍待验证。

### R2 实测失败根因与启动方式对照

用户 R2 输出 secure=true，但 mediaDevices/display 均 undefined。通过 LaunchServices（open -g -j -W -n ... --args --check --report ...）后台启动同一检查二进制、仅改变 Info.plist 的 A/B 实验复现：无用途声明的 Baseline 包接口 undefined；添加 NSMicrophoneUsageDescription 的 Declared 包接口 object/function。此前从 shell 直接执行包内二进制能看到接口的结果不能替代正常 App 启动，之前“加载来源修正已解决”的结论撤回。

新增 build-screen-share-probe.py，复用正式客户端 Info.plist 的媒体用途声明制作 R3；此声明不会主动请求或采集麦克风。正式客户端已有声明，本次不因测试工具问题修改客户端权限。依据：WebKit 缺少有效用途声明时限制媒体能力，https://bugs.webkit.org/show_bug.cgi?id=217104 。R3 仍须用户显式选择屏幕/窗口完成采集验证。

### macOS 手动采集结果与入口调整

用户确认 R3 可以预览窗口，也可通过系统选择器右上角“共享整个屏幕”采集桌面。两个偏好按钮打开同一选择器，不意味着桌面不可用。正式客户端 macOS 设置页改为单一“选择屏幕或窗口”按钮，提示整个屏幕入口；调用 getDisplayMedia 不再传 displaySurface 偏好，交由用户在系统选择器决定。其他平台保留偏好入口，不把偏好描述为强制限制。

前端构建、10 项控制器测试及真实组件交互测试通过；组件测试分别模拟 MacIntel / Win32 平台标识验证 UI 分支，不能代表 Windows WebView2 原生验收。用户尚未反馈系统共享声音结果。未提交、未部署。

### 系统共享音轨的独立验收

画面可见不代表共享声音可用。已请求用户检查 R3 成功结果的 tracks 中是否有 kind=audio（预览默认 muted，不能凭本机无声判定）。当前 WebKit 上游 MediaDevices.cpp 的 getDisplayMedia 路径构建 DisplayMedia 请求时传入空音频约束，与 getUserMedia 路径不同；该源码线索支持需要单独核实系统音频能力，但不把上游版本直接当作本机版本的运行结果。参考：https://raw.githubusercontent.com/WebKit/WebKit/main/Source/WebCore/Modules/mediastream/MediaDevices.cpp 。若实机不提供音轨，当前 WebView 路线仅满足画面共享，完整声音要求仍需原生系统音频采集接入，不能将“仅共享画面”的降级提示视为完成该要求。

### macOS 原生系统声音接入（2026-09-24）

用户授权补充原生采集并由代理自测。新增 `native/system_audio.m` 使用 Core Audio Process Tap（macOS 14.2+），系统混音排除 EdenVoice 主进程的 AudioObjectID；不会采集麦克风，不修改现有通话链路。私有 tap/aggregate device 在停止时销毁。音频线程只写固定容量队列，无法立即取得锁则丢弃本次数据，前端阻塞不会阻塞通话线程。

Rust 以唯一 token 管理采集，校验当前服务器/会话/语音房间；后台监测离房、断线、超过两秒未读取时释放采集。前端通过独立 AudioWorklet 将 PCM 接成 MediaStreamTrack，再使用已有 WebRTC 音频发送与开关。原生音轨初始化晚于界面关闭时立即释放。macOS 使用原生系统声音，其他平台保持系统选择器提供音轨。共享系统声音包含其他应用的播放音频，不承诺仅当前选中窗口的声音。正式包补充 NSAudioCaptureUsageDescription；macOS 14.2 以下请求该能力会明确报错，不能声称支持。

自测使用 Apple Core Audio 真实采集与 AVAudioPlayer/afplay 测试音，仅保存采样计数和 RMS，不保存/上传录音：对照（临时关闭自身排除）自身音 RMS=0.03884、198656 样本；正式排除规则自身音 RMS=0、外部音 RMS=0.12054、190464 样本，48 kHz，停止成功。测试播放器需运行主线程 RunLoop；此前没有 RunLoop 的自身静音结果不足以证明排除，已通过有声对照复验。

浏览器真实 AudioWorklet 测试（native IPC 使用合成 PCM）验证音轨有声、静音/恢复、IPC 失败释放及重复停止；控制器新增原生采集晚返回清理回归。它们与真实 Core Audio 测试是分层验证，尚未证明完整打包客户端通过公网向另一台设备共享系统声音。Windows 原生采集仍未实机验证。

本轮最终回归：桌面 Rust 单元测试 35 项通过；控制器测试 11 项通过；生产前端构建通过；PCM worklet 与共享界面浏览器测试通过。原有真实 WebRTC 编解码/音轨开关/释放回归通过，同期五人合成 UDP 语音应收/实收 5080 包，最大本机转发延迟 699 微秒（仍不代表公网听感）。本轮未提交、未部署。

### 真实 WKWebView 音轨处理器验证

扩展原生测试工具的 `--test-html` 模式，加载由实际 nativeScreenAudio.ts 打包出的测试页，仅用模拟 IPC 产生测试 PCM。经 LaunchServices 正常启动 App 后，在 WKWebView 中验证 AudioWorklet → MediaStreamTrack 的能量、静音/恢复及 IPC 失败释放。结果 `/tmp/eden-audio-worklet-webkit-result.json`：ok=true、engine=WKWebView、energy=0.0064789、muteResume=true、cleanup=true。不录制真实屏幕/系统声音，也不证明正式 Tauri IPC 或跨设备整链路已通过，但消除了此前仅 Chrome 验证音轨处理器的缺口。

## 2026-09-24 Windows code / regression review

- Windows x86_64 MSVC `cargo xwin check --locked --offline` passed. This is target compilation checking, not execution on Windows or installer verification. One pre-existing cross-platform dead-code warning for macOS-only AudioContext fields.
- Legacy runtime codec preference API is optional; when unavailable, SDP negotiates with the VP8-only SFU. ICE negotiation now releases timers/listeners on failure and cancellation, and handles already-complete gathering.
- Controller suite: 14 passed. Production React UI suite passed including Win32 branch, 820x560 layout, audio toggle, collapse/reopen and moderator stop. This uses mocked capture/state, not actual WebView2.
- Go race suites screenshare/control/webadmin passed. Added two independent rooms, rejecting a second publisher in the same room, foreign-room moderator rejection and independent stop/disconnect cleanup.
- Browser/SFU test: first two runs failed before RTP at ICE connectivity (no handshake response); cause remains unconfirmed. Third run passed: 640x360, 8 decoded frames, zero reported video loss, audio off/on and cleanup passed; concurrent synthetic 5-person voice received 4540/4540 expected forwarded packets. Do not describe this as uniformly reliable network or Windows native capture validation. Failure diagnostics now include candidate addresses.
- Windows system-audio capture does not exclude the EdenVoice process. Audio remains off by default and UI explains possible call echo; source availability depends on the system picker. Native Windows capture/audio still needs real device acceptance.
- Current UI: room header strip, in-window video overlay with collapse/fullscreen, no separate native window. Header quality is explicitly a configured ceiling, not measured resolution/fps. One publisher per room, multiple opt-in viewers; different rooms may share concurrently subject to policy limits.

## Embedded screen tab

The Chat header now has room/chat/screen controls. ScreenShare remains mounted during tab switches; the viewer occupies the normal flexible content row (no floating overlay), with object-fit contain and an always-accessible composer/voice dock. Publish/watch selects the screen tab. Switching back keeps tracks and the controller alive; stopping watching explicitly releases them. Fullscreen still uses the video element and returns to the selected tab. Idle/ended screen pages show a placeholder.

Validation: frontend build and 14 controller tests passed; existing settings/moderator UI suite passed. New screen-share-tabs.test.cjs (against the local demo) verifies stream/controller identity and live tracks across room/chat switching, fullscreen enter/exit, no content overflow at 1200x820, 860x660 and 860x580, and track cleanup on stop. Demo uses generated media and simulated state, not native capture.

## Client connectivity deadline

A peer still connecting after 20 seconds or continuously disconnected for 10 seconds now releases local capture and the screen session with a retry message. A recovered connection clears the timer; generation/peer guards prevent callbacks from an old peer affecting a new one. Disposal clears the timer. Controller tests: 15 passed; frontend build passed. Browser/SFU regression after this change passed (640x360, 9 decoded frames, zero reported video loss, audio mute/resume and cleanup); five synthetic voice peers received 4460/4460 expected forwarded packets, maximum local delay 1.466ms. This does not resolve the earlier intermittent ICE failure's unknown cause, or replace Windows native capture and cross-device acceptance.

## Final acceptance — simulation scope approved by user

The user explicitly accepted simulated verification in place of native desktop/window picker and Windows hardware acceptance. The feature is accepted under this scope; earlier native-device gaps are retained as limitations, not represented as completed device tests. No commit, release or deployment is part of this acceptance.

| Requirement | Evidence |
| --- | --- |
| User-selectable resolution, FPS and bitrate | Actual settings component UI tests and fitScreenSettings boundaries; fresh browser/SFU runs with 1280x720/20fps/1800kbps and 1920x1080/30fps/4000kbps settings both decoded the requested resolution. FPS/bitrate are requested ceilings, not claims of measured sustained output. |
| Server-enforced limits and audio permission | Settings.Validate, actual RTP guard tests, policy permission/persistence/conflict tests; screenshare/control/webadmin race suites passed on current source (cached unchanged Go code). |
| Desktop/window source choice | Actual Windows-branch selector interaction and source argument tests; macOS combined system picker entry tested. Native OS selection remains outside simulated acceptance. |
| Shared audio switch | Both browser/SFU runs decoded synthetic tone, verified off/on, and released tracks; actual PCM worklet with simulated IPC passed tone, mute/resume, failure cleanup and idempotent stop. |
| Room isolation and concurrent rooms | Manager tests cover rejected foreign-room subscription/moderation, one publisher per room, independent simultaneous rooms, departure cleanup. |
| Embedded responsive tab and fullscreen | Actual Chat/ScreenShare components in demo passed room/chat/screen switching with identical controller/live tracks, fullscreen entry/exit, compact sizes and explicit stop cleanup. |
| Failure/cancellation handling | 15 controller tests passed including late capture, stale responses, disconnected deadline, canceled ICE and native audio cleanup. |
| Build compatibility | Fresh frontend build passed; Windows MSVC target check passed earlier, no Rust changes since that check. |

Fresh runs: 720p decoded16 frames / loss0, concurrent voice3220/3220 packets max local565µs; 1080p decoded13 frames / loss0, voice3120/3120 max703µs. These are short local synthetic tests, not public-network capacity or Windows-native capture claims. Earlier intermittent ICE handshake failures remain documented; bounded client failure cleanup is present, and their root cause has not been established.

Current interactive preview: `node desktop/scripts/screen-share-demo.mjs` (127.0.0.1:9198). UI is the real component; media/control state is simulated. Windows system-wide audio may include call playback; audio defaults off, with an explicit UI note. macOS native system audio requires14.2+.
