# 语音链路第二轮优化（2026-09-10）

基于 `5ed400f`，落实第二轮检查的五项工作。仅本地修改与暂存，不提交、不推送、不部署。

## 实现

1. **相位对齐追帧**：在 10–20 ms 偏移范围搜索归一化相关性，再做 20 ms 交叠渐变。保留未消费的 PCM 尾部，维持下一帧边界连续；沿用积压触发和每 25 tick 最多一次的追帧限制。150/175/200/225/440 Hz 回归要求输出能量至少保留 90%，并验证首尾样本连续。
2. **采集 DSP 与播放隔离**：新增 `voice-capture-dsp` 线程，拥有降噪、AEC、AGC、VAD 和处理器切换。网络收发、解码、混音和播放排队不等待采集处理。处理结果队列最多 2 帧，采集排队超过 80 ms 丢弃、完成后软件总年龄超过 120 ms 不发送。设备输入延迟与软件排队时间分开，250 ms 硬件延迟的测试帧仍可正常进入处理器。显式阻塞 DSP 的测试验证播放连续输出。
3. **AEC 参考与时序**：播放回调消费音频时才提交参考，包括欠载渐出帧；未被播放队列消费的混音不会进入参考。根据 CPAL 输入/输出时间戳和帧偏移估计延迟，约束在 0–500 ms，传入 WebRTC、Sonora 以及神经降噪前置 AEC。采集线程独占 AEC 状态，回调只做有界、非阻塞入队。
4. **诊断可见性**：桌面诊断显示采集帧年龄、实际 jitter 加剩余 PCM 缓冲、软件播放队列时长、工作线程 P95/P99、播放调度迟到 P99、采集处理 P95、过期丢帧、DSP 异常静音帧、AEC 延迟及参考丢帧。每秒统计一次；时间直方图分辨率 1 ms，200 ms 及以上合并到末桶。停止音频时清除新增诊断状态。发送帧的 FEC 比例检查实际 LBRR 标志，接收侧继续区分 FEC 与 PLC。
5. **真实语音 FEC 验证**：增加可复现、默认忽略的 `real_speech_fec_matrix` 测试，比较 FEC 开关、6 个码率、自动/宽带、3 个预期丢包档位、随机与突发丢包。测量未显示强制宽带或降到 24 kbps 能提高本样本恢复率，因此保留原自适应 96/72/48/32 kbps 策略和用户固定码率，避免无证据地降低带宽。

严格 Clippy 同时修复桌面端已有的条件合并和复杂类型警告，未增加警告豁免。

## FEC 数据与复现

公开样本来自 [Opus 官方示例](https://www.opus-codec.org/examples/)，由其页面脚本引用 [Xiph 原始语音 WAV](https://media.xiph.org/opus/samples/examples/speech_orig.wav)。48 kHz、单声道、16-bit、10.8 秒，共 540 个 20 ms 包。WAV SHA-256：`625ae47a791214ca5cef9927ac51ea085158a5ca1e3cc77db019b15579d63aa5`。音频仅下载到临时目录，仓库保存测试和 [288 组原始结果](voice-fec-matrix-2026-09-10.csv)。

随机丢包使用固定种子 LCG，前 6 帧保留初始化；短样本的实际丢包比例不等于配置概率。突发模式每 30 帧连续丢 3 帧。解码逐帧检查有限值，只有已有 SILK 状态且下一可用包带 LBRR 时计入恢复，其他为 PLC。

| 自动带宽配置 | 场景 | 丢包 | FEC 关闭：PLC | FEC 开启：恢复 / PLC |
|---|---|---:|---:|---:|
| 72 kbps，预期损失 5% | 随机 5% | 20 | 20 | 12 / 8 |
| 48 kbps，预期损失 10% | 随机 10% | 35 | 35 | 19 / 16 |
| 32 kbps，预期损失 20% | 随机 20% | 81 | 81 | 49 / 32 |
| 32 kbps，预期损失 20% | 连丢 3 帧 | 51 | 51 | 14 / 37 |

这些是该样本的冗余恢复数量，不是主观音质评分或所有语音的收益保证。16 kbps 在该矩阵没有可确认恢复；突发丢包仍依赖 PLC，不能把 FEC 开关视为消除丢包。

```bash
curl -fL https://media.xiph.org/opus/samples/examples/speech_orig.wav -o /tmp/edenvoice-speech-orig.wav
python3 - <<'PY'
import wave
with wave.open('/tmp/edenvoice-speech-orig.wav') as w:
    assert (w.getframerate(), w.getnchannels(), w.getsampwidth()) == (48000, 1, 2)
    with open('/tmp/edenvoice-speech.pcm', 'wb') as out:
        out.write(w.readframes(w.getnframes()))
PY
CMAKE_POLICY_VERSION_MINIMUM=3.5 EDENVOICE_SPEECH_PCM=/tmp/edenvoice-speech.pcm \
  cargo test --manifest-path client/Cargo.toml --lib --offline real_speech_fec_matrix -- --ignored --nocapture
```

## 验证与边界

- 客户端 256 项单元测试、1 项 Slint UI 集成测试及全部示例目标通过（UI 测试编译需 `SLINT_EMIT_DEBUG_INFO=1`）；默认忽略真实语音矩阵和五分钟压测，二者分别运行通过。
- 桌面 Rust 32 项测试通过；客户端和桌面 `cargo clippy --all-targets --offline -- -D warnings` 通过；React/TypeScript 生产构建通过。
- 五人、模拟五分钟 Opus 流水线：最大缓冲 3 帧，2 次 20 ms 处理预算超时，峰值 27.24 ms；这是本机 debug 模拟测试，不是实机通话延迟测量。
- AEC 参考是输出回调消费的 48 kHz PCM，位于设备重采样/格式转换之前。设备时间戳估计不包含未知声学传播、重采样滤波器群延迟或设备固件未报告的延迟。未做双机通话、蓝牙切换或扬声器回声实测，不能声明声学效果已验证。
- DSP 隔离使单次采集阻塞不直接阻断播放；系统整体 CPU 饱和、设备驱动卡顿仍可能影响所有线程，可通过新增播放调度指标区分。
