# 完整语音链路复查：SFU / P2P / 自适应（2026-09-10）

排查基线：`81b8df7`。以下六项保留修复前的位置与复现证据；随后已按要求完成修复，最终实现与验证见文末“修复结果”。

## 1. P1：控制消息跨读取超时后丢失分片，导致错误重连

位置：`client/src/control.rs:1041`，调用处 `:721`，100 ms 读超时设置 `:469`。

`read_limited_line` 已消费部分 TCP 字节后遇到 TimedOut/WouldBlock，主循环继续调用该函数；函数入口 `output.clear()` 清除之前的半行。后半行作为独立 JSON 解析失败，进入 ConnectionLost，应用随后停止原音频会话并重连。网络分片、丢包重传或较大的状态消息均可能触发。

复现：构造“JSON 前缀 → TimedOut → 后缀与换行”的 BufReader。第一次调用保留前缀但返回超时；第二次仅得到 `"data":{"nonce":1}}`，JSON 解析失败。用例 `review_fragment_timeout_loses_json_prefix` 确认。

修复方向：跨超时保存未完成帧，只有交付完整消息后才能清空；保留总长度限制，并让心跳与取消命令仍可执行。

## 2. P1：同一会话重建音频传输时，序列号归零导致注册和语音被拒收

位置：`client/src/transport/adaptive.rs:183`、`:209`、`:250`；触发入口 `client/src/application.rs:544`、`:550`；替换引擎 `client/src/audio.rs:363`。

应用在 Starting 状态修改音频设置，或设备错误后改设备，会重新创建 AdaptiveTransport，但仍复用同一 session 和 MediaSecurity。构造器总以 sequence 0 注册、重新从 1 发包。服务端对应会话及 P2P 对端的防重放窗口仍记得旧序列号，因此新端点注册及新音频被拒绝，直到序列号追上旧窗口或会话真正重连。新 socket 注册失败还会维持服务器的旧目标地址。复用同密钥、epoch、序列号也会重复使用加密 nonce，不能通过放宽防重放来修复。

复现：先接受同一会话的 sequence 100，再使用相同 MediaSecurity 构建真实 UDP transport。新注册 sequence 0、新音频 sequence 2 均被原接收 cipher 拒绝为 replay/stale。用例 `review_audio_restart_reuses_sequence_and_registration_is_rejected` 确认。

修复方向：媒体序列号、时间戳和换钥状态归属会话生命周期；设备/音频引擎重建不能重置。或者明确进行新会话/新密钥协商后再发包。

## 3. P1：UDP 重新解析 DNS 第一项，没有继承实际连接成功的控制端地址

位置：`client/src/transport/adaptive.rs:1842`；TCP 地址尝试 `client/src/control.rs:374`。传统 Relay 的解析函数也相同。

控制连接会遍历所有解析地址并尝试连接，UDP 仅选 `.next()`。服务端广告通配地址时，又对控制域名解析第一项；这个地址不一定是 TCP 实际连接成功的地址。双栈环境 IPv6 不通、IPv4 可用时，控制已连上但语音仍可能发向不可用 IPv6；多地址 DNS 也有相同风险。维护线程沿用同一个 server 地址，重建 socket 不能纠正它。

复现：localhost 同时返回 IPv6/IPv4，仅启动 IPv4 TCP listener。TCP 选中 `127.0.0.1`，UDP 解析选中 `[::1]:9001`。用例 `review_media_resolution_ignores_connected_control_endpoint` 确认。

修复方向：将实际成功的 TCP peer 地址传给媒体层作为通配广告的回退地址；独立媒体域名需要候选地址探测/失败切换，而不是静默固定第一项。

## 4. P2：发送路径仍会等待共享加密锁，拖住解码和混音

位置：`client/src/transport/adaptive.rs:1042`；调用入口 `:280`。

socket、路由和 pairwise context 已使用 try_lock，但 SFU 包加密仍调用 `security.outbound.lock()`，且发生在 P2P 分支之前。该锁也供注册续期、服务端探测及换钥使用。持锁线程如果被调度暂停，调用 send_audio 的音频工作线程会一起等待，从而影响接收解码和播放。纯 P2P 也无法避开这一步。

复现：真实 transport 建立后占住 outbound cipher 锁，启动 send_audio；80 ms 内未返回，释放后才完成，测得约 80.53 ms。用例 `review_outbound_cipher_lock_blocks_media_sender` 确认。这是受控持锁故障注入，不是正常加密耗时测量。

修复方向：热路径无等待获取加密状态，或将加密/发送置于有界独立任务中；保持 nonce 分配和换钥原子性，不以共享同一 nonce 的独立 cipher 副本替代。

## 5. P2：失效候选仍保留 Qualified，P2P 回退逐条试过期地址

位置：`client/src/transport/adaptive.rs:1446`、`:1481`；稳定探测间隔 `:855`。

稳定期只探测最优候选，其他曾合格候选不更新探测时间。最优路径失效后，每个探测周期只降级一条，然后立即选另一条 Qualified 旧候选。send_to 成功只代表本地发出，纯 P2P 因此仍可能跳过 SFU 副本，造成切换间隙断音。

复现：同一 peer 设置 6 条都已超过超时的 Qualified 候选。连续调用 send_probes，前 5 次仍返回 P2P，第 6 次才回到 SFU。用例 `review_expired_alternate_routes_keep_p2p_selected` 确认。实际断音时长还受服务端报告 3 秒过期和路由广播影响，不能简单推算为无限等待。

修复方向：备选直连必须有新鲜成功探测；所有可选路径都过期时立即使用 SFU，并继续后台验证候选。

## 6. P2：P2P 重获资格使用累计丢包率，历史故障长期阻止直连恢复

位置：`client/src/transport/route.rs:144`、`:150`；探测统计首个 ACK 才重置 `client/src/transport/adaptive.rs:1650`。

失效降级并不清空 lifetime sent/received。虽然降级检测使用近期窗口，重新资格判断仍使用累计损失。短时断网积累的丢包会一直保留，网络恢复后也可能需要远超 qualification_ms 的时间才能回到 P2P。如果 SFU 路径本身较差，用户会长期留在更差路径。

复现：先 300 次健康 ACK 获得资格，模拟 100 次丢包，再持续 60 秒、800 次无丢包 ACK。当前 RTT 10 ms，但累计丢包仍 8.33%，持续不合格。用例 `review_old_probe_loss_blocks_requalification_after_recovery` 确认。

修复方向：重新资格评估使用有足够样本的近期窗口或新的测量周期，保留防抖与最短稳定时间。

## 排查阶段覆盖范围与验证

- 阅读控制连接、心跳/重连、注册续期、socket 更换、候选交换、P2P 资格/降级、SFU 回退、加解密、防重放、媒体去重、音频线程衔接及设备恢复代码。
- 阅读服务端 SFU worker 分片、有界队列、过期丢帧、writer 写超时、UDP 源验证、房间路由评估和控制广播。
- 6 个临时客户端复现用例均确认问题行为；日志保存在 `/tmp/edenvoice-review-repros.log` 和 `/tmp/edenvoice-review-extra.log`。源码已经恢复，未将证明缺陷存在的断言混入正常测试。
- 使用已缓存 Go 1.23.0 和匹配 GOROOT 执行 `go test -race ./internal/voice ./internal/serverstate ./internal/control`，三个包均通过。未在本轮找到新的服务端已复现缺陷；测试通过不能覆盖上述跨组件故障。
- 正常桌面连接全部走 AdaptiveTransport，包含 force_sfu。传统 RelayTransport 还存在仅启动注册一次、接收错误直接退出、队列满丢新保旧的问题，但它不等同于当前桌面的正常 SFU 路径，本轮未把它们混入以上六项主要发现。
- 未做跨公网双机、真实 NAT/防火墙、蓝牙设备或生产环境验证；没有推送或部署。

建议修复顺序：控制分片、会话媒体序列连续性、地址选择，然后消除加密等待并修正 P2P 候选新鲜度与重新资格统计。


## 修复结果

六项均已落实，另修复加密 P2P 全互联验证中发现的收发锁竞争丢帧。代码及报告仅暂存，未提交、推送或部署。

- 控制行读取跨 TimedOut/WouldBlock 保留分片，完整行交付后才清空；半条消息遇 EOF 报错结束，避免重复交付。保留 64 KiB 上限。
- TCP 成功连接的 peer 地址以控制事件传入应用，供音频启动使用。通配媒体广告继承该地址；媒体域名候选优先匹配成功连接的 IP/地址族，独立显式 IP 仍尊重广告配置。Relay 与 Adaptive 共用解析规则。
- AdaptiveControl 保存会话级 sequence、timestamp、换钥状态与方向独立的 P2P 加密上下文；音频重建后的注册、探测、音频继续分配唯一序列号。没有放宽接收防重放检查。
- SFU outbound cipher 改为非阻塞获取；纯加密 P2P 成功发送时不再生成 SFU 密文。锁忙返回 WouldBlock，由调用方按实时帧处理，不等待持锁线程。媒体路径统计也改为非阻塞读取。
- 选路检查最近 ACK 时间，并在探测周期中一次性降级全部过期候选；新鲜备用候选仍可接管。全部过期时即使用 SFU。
- 重获资格使用最多 8 秒、128 个采样点的近期丢包窗口，保留资格持续时间和降级防抖；上报服务端的 loss 与该窗口一致。
- 加密全互联首次验证只收到 396/400 次投递，定位到收包复用了发送侧 try_lock 密钥上下文。现在发送只操作发送上下文、接收只操作接收上下文，避免正常全双工流量因对向加密忙而丢包；不共享同一方向的 nonce 分配器副本。

回归用例：

1. `fragment_timeout_preserves_json_prefix`
2. `consecutive_lines_and_partial_eof_are_handled_once`
3. `restarted_audio_keeps_session_sequence_timestamp_and_registration_valid`
4. `wildcard_media_address_uses_connected_control_peer`
5. `busy_outbound_cipher_does_not_block_media_sender`
6. `expired_alternate_routes_fall_back_to_sfu_immediately`
7. `expired_qualification_is_not_selected_between_probe_ticks`
8. `recent_loss_free_window_allows_requalification`
9. `encrypted_direct_receive_survives_busy_outbound_contexts`

验证：客户端全部目标测试（265 项单元测试、1 项 Slint UI 集成测试、3 项示例测试）通过；默认忽略项未计入通过数量。客户端与桌面端 `cargo clippy --all-targets --offline -- -D warnings` 通过。服务端 SFU、房间状态、控制连接的 Go race 测试通过。

加密 P2P 全互联执行 `cargo run --example p2p_send_budget -- 2 6`，2 人与 6 人各 3 轮：每轮分别 400/400 与 6000/6000 次投递，总计 19,200 次全部收到、无重复；少量 SFU 回退仍成功投递。该数据来自本机 debug 回环测试，不代表公网或麦克风到扬声器的端到端表现。
