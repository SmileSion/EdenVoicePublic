# 实时语音链路检查（2026-09-09）

最初完成代码审查和本地自动化验证；随后按要求修复以下四项，修复与验证记录见文末。覆盖采集/重采样、DSP、Opus、UDP/P2P/SFU、接收缓冲/混音、设备生命周期和控制连接恢复。没有进行真实设备拔插、双机听感或公网故障注入；测试通过不等于这些场景已经通过验收。

## 1. P1：UDP 阻塞发送可拖住双向音频

位置：client/src/transport/adaptive.rs:345-369，connect:182，rebuild_udp_sockets:1299；client/src/audio.rs:1587。

正常和重建的 UDP socket 只配置 read_timeout，没有 write_timeout 或非阻塞发送。send_audio 在唯一的音频 worker 上直接 send_to，且持有共享 socket Mutex。OS 发送缓冲拥塞或网络驱动发送停顿时，调用没有应用层时间上限；这段时间接收队列消费、解码和混音均不能执行。音频 worker 的 next_mix 截止检查无法中断已经进入的 send_to。修复 socket 的接收线程需要取得同一把锁，因此也无法及时替换它。

建议将发送放入独立、有界且淘汰过期帧的发送器，或为发送实现短时限并正确处理 WouldBlock/TimedOut；P2P、SFU、探测及重建 socket 都需覆盖。已从代码确认缺少限时；本次未在真实网络驱动上复现 send_to 阻塞。

## 2. P1：控制连接只发心跳，不判心跳超时

位置：client/src/control.rs:676-687、703-709、848。

pending_ping 只用于 RTT，下一次 Ping 会覆盖尚未响应的记录；100ms read timeout 被忽略，没有“连续多久未收到响应就断开”的判断，也没有设置写超时。若切网/NAT 映射失效或服务器失联但连接没有及时收到 FIN/RST，客户端不能按心跳时限触发 ConnectionLost 和重连，恢复取决于 TCP 最终报错。界面和媒体仍可能绑定旧 session。服务器侧清理会话不能保证关闭通知穿过黑洞网络。

建议保留未决心跳的截止时间，超过合理失联阈值主动退出 run_session，进入现有重连和频道恢复流程；写入也应有时限。代码路径确认；本次未进行真实 TCP 黑洞实验。

## 3. P2：抖动队列积压不会随网络恢复收敛

位置：client/src/jitter.rs:147-164；client/src/audio.rs:1625-1627、1666、1789-1800。

set_network_quality 降低目标时只修改 prefill_frames，已经启动的 pop 始终每次消费一帧。playback_cadence 只校正硬件播放队列深度，不校正各说话者的 jitter 深度。持续语音按每20ms一帧到达和消费时，增加的缓冲不会消退，指标中的 target delay 却已经降低。

直接使用当前 jitter.rs 的独立 Rust 复现结果：

- Critical 预缓冲8帧，播放启动后改为 Good（目标3帧），连续1500次正常收发后仍积压7帧，即140ms。
- 正常3帧预缓冲后模拟160ms停顿，追加8帧（未超过单批 MAX_NETWORK_CATCHUP_FRAMES_PER_PEER），恢复1500次正常收发后仍积压9帧，即180ms。

以上为 jitter 队列本身的时长，不含采集、编码、网络、DSP和设备播放延迟；30秒是1500个20ms周期的模拟时间。说话间隔导致重置时可以恢复，但连续通话无法自动恢复低延迟。

建议设置基于实际 jitter 深度/帧龄的受控追赶策略，并在网络恢复降低目标时逐渐减少积压；避免直接大量丢帧产生新的听感断裂。复现源码保存在 /tmp/edenvoice-voice-review/jitter_repro.rs。

## 4. P2：音频设备启动失败会留下无法停止的 worker

位置：client/src/audio.rs:774-776、990-993。

AudioEngine::start 先 spawn worker，随后执行 input_stream.play()? 和 output_stream.play()?。若其中一步失败，函数在构造 AudioEngine 前返回，因此 AudioEngine::drop 不会执行，alive 保持 true。worker 持有 alive、transport 和 stale_playback 等对象，而且 run_worker 没有因 capture/commands 断开而退出的分支，因而线程、媒体 socket 和维护线程可以持续存在。再次尝试启动会累积资源；使用相同 session 重试时，遗留 transport 的注册保活还可能干扰新 socket 的服务器地址绑定。

建议在启动失败路径显式取消并回收 worker，或使用构造期清理守卫，保证每条失败路径都释放媒体资源。此项为明确的控制流缺陷，尚未用真实设备注入 play() 失败。

## 验证

- Rust：cargo test --lib --offline，246通过，0失败，1个默认忽略的压力测试；首次沙箱运行的12项失败均为本地端口监听被禁止，放开本地测试权限后全部通过。
- Go：mise exec -- go test -race ./internal/voice ./internal/serverstate ./internal/control，三个包全部通过。
- 独立 jitter 复现：两种场景均确认积压持续存在。
- 五路五分钟模拟压力测试通过（15000个20ms周期，实际执行约22.54秒）。

优先处理发送时限和失联检测，再处理播放积压与初始化失败清理。现有五路编码/混音测试并不执行真实 CPAL 回调和完整 run_worker 调度，因此不能排除硬件或线程调度引起的卡顿。

## 修复记录

四项已修复，按用户要求只暂存，不提交、不部署。

1. 所有 Adaptive 初始/重建 socket 和 Relay socket 使用非阻塞模式；音频发送取得共享 socket 时改用 try_lock，繁忙时丢弃当前帧而不阻塞混音。接收端空读暂停2ms，避免空转；临时 WouldBlock 不触发无意义的 socket 重建。
2. 控制 TCP 写入限时5秒；未决 Ping 不再被下一次心跳覆盖，20秒未收到匹配 Pong 时关闭旧连接并进入现有重连流程。限制每轮处理的控制命令数，避免持续命令流饿死超时检查。
3. 各 RemotePeer 根据真实 jitter 深度渐进追赶：持续超出目标且接下来两帧连续可用时，最多每25个播放周期将两帧解码后交叉淡化合为一帧。保留 Opus 顺序状态、不跨越缺包、不低于当前安全余量。模拟停顿和弱网恢复场景在6秒正常流量内收敛，未制造额外 PLC 或丢包计数。原独立 jitter 复现仅操作缓冲容器；现在实际追赶由 RemotePeer 完成，因此以新的 peer 回归测试为准。
4. 在线程创建前建立 AudioWorkerLifetime 清理守卫；设备启动提前返回时自动取消 worker，成功时守卫由 AudioEngine 持有，退出同样取消。测试验证输入/输出启动失败路径均能使后台资源释放。

修复后验证：

- cargo test --lib --offline：251通过、0失败、1个默认忽略的压力测试。
- 五路五分钟模拟压力测试单独运行并通过。
- 新增测试包含 socket 交接锁竞争、重建 socket 非阻塞、无匹配 Pong 的连接失联、播放延迟收敛/时间戳回绕、缺包时不追赶，以及启动失败资源释放。
- 严格 Clippy 未全绿：既有 application.rs/model.rs 的可折叠条件、config.rs 测试的默认值后赋值、complete_identity_handshake 的参数数量仍被 -D warnings 拦下；诊断未指向此次新增实现。
- 未进行真实麦克风/扬声器双机听感、设备拔插和公网黑洞验证。交叉淡化追赶的实际听感仍需硬件验收。

## 2026-09-10：严格 Clippy 告警修复

已修复上文记录的全部既有 Clippy 告警：合并 application.rs/model.rs 的冗余条件和 match 分支条件，使用结构体初始化设置测试配置，将握手专用临时缓冲区移入 complete_identity_handshake，减少其参数数量。未新增 lint 豁免。

验证结果：

- `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo clippy --manifest-path client/Cargo.toml --all-targets --offline -- -D warnings` 通过，零告警。
- `cargo test --lib --offline`：251通过、0失败、1个压力测试默认忽略；该压力测试此前已单独通过。
- 差异检查通过。按用户要求仅暂存，未提交、未部署。
