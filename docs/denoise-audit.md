# 麦克风降噪链路审计

本文记录 EdenVoice 0.6.6 的三条麦克风降噪链路、离线回归方法和本次修复结论。离线入口位于
`client/examples/denoise_regression.rs`，它直接复用实时链路的 `CaptureAudioProcessor`，但绕过设备、
监听、VAD/DTX、Opus、网络、播放和 UI。

## 运行方法

输入必须是 48 kHz、单声道、PCM16 或 IEEE-float WAV：

```bash
cd client
cargo run --release --example denoise_regression -- input.wav output-directory
```

输出为 32-bit float 无压缩 WAV：

- `*-original.wav`
- `*-webrtc-apm.wav`
- `*-rnnoise.wav`
- `*-deepfilternet.wav`
- `*-metrics.tsv`

离线 WebRTC 测试禁用 AEC（因为没有时间对齐的 render reference），保留标准模式的中等 NS、HPF、
transient suppression 和保守 AGC2。RNNoise 与 DeepFilterNet 测试关闭可选 AEC/AGC，从而测量模型本身，
全程为 100% processed、0% dry。DeepFilterNet 实时链路固定处理延迟约 70 ms；离线导出补足尾部并
移除开头 3,360 samples 来对齐，输出长度与输入完全相同。

生成七组确定性回归素材：

```bash
cargo run --release --example denoise_regression -- --generate-fixtures output-directory
```

素材覆盖持续背景噪声、杯盖瞬态、键盘瞬态、单独人声、人声加杯盖、人声加键盘、说话间隙瞬态。
其中人声与噪声是确定性合成信号，用于自动化与数值回归；真实听感验收另外使用用户提供的杯盖录音。

## 实际链路

公共采集链路：

```text
CPAL 设备 PCM（设备原生采样率/格式/声道）
→ cpal Sample 转换为归一化 f32
→ 多声道等权下混为单声道
→ band-limited sinc 重采样到 48 kHz
→ 聚合为 960 samples / 20 ms
→ 用户输入增益（一次，限制到 [-1, 1]）
→ 所选处理器
→ VAD 仅决定是否发送，不改变 PCM
→ Opus 直接编码处理后的同一个 960-sample frame
```

麦克风 A/B 自听同时保存 `original_frame` 和处理后的 `frame`，不会将原声混入网络发送。模式切换时有
明确的 5 帧旧处理器到新处理器交叉淡化，只在切换瞬间生效；离线回归不经过该逻辑。

### 标准模式（WebRTC APM）

```text
48 kHz mono f32
→ 每个 960-sample frame 拆为两个连续 480-sample / 10 ms chunk
→ HPF
→ AEC3（实时模式有 render reference；离线禁用）
→ WebRTC NS（默认 Moderate）
→ transient suppression
→ 保守 AGC2
→ Opus
```

使用 `webrtc-audio-processing-sys 2.1.0` 的 bundled `webrtc-audio-processing 2.1`。实时标准模式按产品定义
始终启用 AEC3 与 AGC2。render reference 是最终送播放设备的混音，当前 delay 使用 60 ms 的保守估计；
不同声卡/蓝牙设备的真实声学路径会变化，这是当前 AEC 精度仍存在的设备相关边界。

### RNN 模式（RNNoise）

使用 `rnnoise2 0.1.2` 编译的 Xiph RNNoise C 引擎与固定模型
`0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37`。这与
`pyrnnoise 0.4.3` wheel 的模型代际一致，而不是旧版 `nnnoiseless 0.5.2` 的小型模型。原生接口要求
f32 容器内的 PCM16 数值尺度，而非归一化尺度。EdenVoice 的实际转换是：

```text
[-1, 1] × 32768
→ 同一个 native Denoiser 连续处理两个 480-sample chunk
→ ÷ 32768
→ [-1, 1]
```

speech probability 没有用于原声旁路；每一帧都 100% 使用 RNNoise 输出。可选 AEC 位于 RNNoise 前，
可选 AGC 位于 RNNoise 后。

### Deep 模式（DeepFilterNet）

依赖固定在 `deep_filter 0.5.6`、Git revision
`978576aa8400552a4ce9730838c635aa30db5e61`，启用 bundled `DeepFilterNet3_onnx` 默认模型。模型运行时已验证
48 kHz、单声道、hop size 480；一个 20 ms frame 连续调用两次同一个 `DfTract` state。模型自身的 STFT
和 lookahead 延迟为 1,440 samples（30 ms），worker 另有固定两帧（40 ms）调度延迟，总处理延迟约
70 ms；超时或缺帧时输出静音，绝不回退到原声。

自然模式为 18 dB attenuation limit；强力模式为 100 dB attenuation limit，并启用 beta 0.02 post-filter。
可选 AEC 位于模型前，可选保守 AGC 位于模型后，没有叠加 WebRTC NS 或 RNNoise。

### DeepFilterNet 版本复核

- 当前固定的 `v0.5.6`（commit `978576aa8400552a4ce9730838c635aa30db5e61`）仍是上游最新稳定发布。
- 客户端加载的是完整质量的 `DeepFilterNet3_onnx.tar.gz`，不是 low-latency 模型；归档 SHA-256 为
  `c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616`。
- 上游当前 `main` 标记为 `0.5.7-pre`。稳定版与 `main` 的全部四个模型归档逐字节相同，因此不存在
  RNNoise 审计中发现的同名不同模型问题。
- `main` 的相关变化是 Tract 从 0.19 升至 0.21，以及连续近静音 frame 的 skip-counter 修复；没有新模型、
  新权重或可证明的降噪质量升级。未发布分支的宽松依赖范围在独立消费时还会解析到不兼容的新 Tract，
  因此当前不以牺牲三平台可复现构建为代价追踪 `main`。
- 同一杯盖录音中，自然模式受 18 dB attenuation limit 约束，RMS 从 `0.01281611` 降至 `0.00161562`；
  强力模式将该段判定为非语音并输出静音。残留差异来自模式参数，而不是旧模型或错误推理路径。

## 根因与修复

1. **幅值缩放正确，但模型选错了。** 旧实现确实执行 `[-1,1] × 32768`，分帧与 state 复用也正确；然而
   `nnnoiseless 0.5.2` 使用的是旧式小模型，不能作为 `pyrnnoise 0.4.3` 的等价参考。后者捆绑的原生库
   包含新版 Conv2D/GLU/GRU 网络。现在客户端已替换为相同 Xiph 模型代际。
2. **严格同输入 A/B 已通过。** 同一个 FFmpeg 生成的 48 kHz mono PCM16 WAV（SHA-256
   `df9d683d9138f9999d2134efb4be3507756732935d344a21f0499130e8caa6a3`）下，`pyrnnoise 0.4.3` 输出
   `-71.7781 dBFS RMS / -36.6121 dBFS peak`；替换后的 EdenVoice 输出约
   `-71.98 dBFS RMS / -36.75 dBFS peak`。量化到 PCM16 后两者相关系数为 `0.9993915`，最大逐样本差
   20 个 PCM16 LSB；差异属于编译器/浮点实现，不是处理链差异。
   首个 480-sample frame 恰好是编码器前导静音，两边入口和出口的 min/max/RMS 都为 0。输入 RMS 最大的
   frame 1100 在 RNNoise 入口为 `min=-11852, max=10052, RMS=3550.23`，证明传入的是 PCM16 数值尺度；
   `pyrnnoise` 出口为 `min=-2, max=2, RMS=0.5303`、speech probability `0.1119046`，EdenVoice 出口为
   `min=-1, max=1, RMS=0.3129`。
3. **发现神经降噪的 AGC 顺序不合理。** 旧 RNNoise/DeepFilterNet 在可选 AGC 开启时先执行 AGC，再执行
   模型，会先放大待消除噪声。现在顺序改为 `AEC → model → conservative AGC`。
4. **发现设备切换保留旧模型状态。** 输入/输出设备切换或故障恢复后，旧处理器 state 会继续使用。
   现在成功切换或恢复设备后，通过后台 loader 按当前设置创建全新处理器，避免旧噪声统计、AEC reverse
   stream 和固定延迟队列残留。

## 真实杯盖录音结果

原始 M4A（24 kHz mono AAC）只在准备基准时解码、重采样一次为统一的 48 kHz mono float WAV。四路都读取
同一份 616,448-sample 输入，时长 12.843 秒：

| 模式 | RMS | 峰值 | RMS 相对原声 | 处理耗时 | RTF |
|---|---:|---:|---:|---:|---:|
| 原声 | 0.01281611 | 0.68002319 | 0.00 dB | 1.750 ms | 0.000136 |
| WebRTC APM | 0.01225426 | 0.56188756 | -0.39 dB | 29.158 ms | 0.002270 |
| RNNoise | 0.00025191 | 0.01453578 | -34.13 dB | 604.951 ms | 0.047105 |
| DeepFilterNet natural | 0.00161562 | 0.08574305 | -17.99 dB | 372.106 ms | 0.028974 |

所有文件长度一致，clipped samples 和 non-finite samples 均为 0。新版 RNNoise 对这份以杯盖声为主、
几乎没有有效人声的素材实现约 34 dB RMS 衰减，与用户给出的参考行为一致。它仍不是严格的人声分离器；
人声和瞬态同时出现时的保留程度必须继续用真实通话素材听测。

这次 RNNoise 模型更大，当前样本的 wall-clock RTF 为 `0.047105`，仍低于实时预算但不再沿用旧小模型的
性能数字。这不是跨平台 CPU profiler 结果，Windows、Intel 与不同电源策略会不同；三平台 release 构建
仍需保留性能与包体积门禁。

## 自动化验证

- passthrough 在 unity gain 下逐样本完全一致。
- 所有四路输出等长、finite、范围在 `[-1, 1]`，且三种处理结果两两不同。
- RNNoise 每 960 samples 调用两次 480-sample 处理，并持续复用同一个 state。
- RNNoise 逐样本匹配 native PCM16-scale contract，且使用与 `pyrnnoise 0.4.3` 相同哈希的模型。
- RNNoise 不混入原声；DeepFilterNet 超时/缺帧只替换静音。
- AEC/model/AGC 顺序有结构测试。
- 新建/替换处理器从全新算法 state 开始。
- Opus handoff 的自动化测试只编码最终 processed buffer。
- 44.1/48/24/16 kHz 重采样、voice-band 能量和 Nyquist alias rejection 有测试覆盖。

仍需真实设备听测的部分包括：扬声器到麦克风的 AEC 声学路径、蓝牙系统缓冲延迟、具体麦克风阵列的
声道布局，以及 DeepFilterNet 对不同说话人音色的主观损伤。RNNoise 和 WebRTC NS 都不是严格的人声分离器，
人声同时出现的杯盖、玩具喇叭、音乐或其他拟人声瞬态无法靠工程参数保证完全消除。
