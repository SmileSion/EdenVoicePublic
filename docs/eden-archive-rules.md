# Eden Archive 声明式记忆节点

Eden Archive 使用两层规则：复杂彩蛋继续由 Go `Predicate` 实现；普通里程碑和记忆节点可以通过 JSON 声明，无需重新编译或重启服务端。

## 配置

```toml
[archive]
enabled = true
rules_file = "data/eden-archive-rules.json"
```

`rules_file` 省略时默认为 `<security.data_dir>/eden-archive-rules.json`。服务端每秒只检查文件元数据；内容变化后才读取、校验和编译。非法更新不会替换当前生效规则，修正文件后会自动恢复加载。

首次启动时无规则文件表示仅使用内置规则；首次启动配置无效会明确报错。`enabled = false` 是硬关闭，不读取规则文件、不启动 Archive 后台任务。启停总开关仍需重启服务端，规则内容更新不需要。

完整示例见 [`server/archive-rules.example.json`](../server/archive-rules.example.json)。

## 规则结构

```json
{
  "schema_version": 1,
  "revision": 1,
  "rules": [
    {
      "id": "hundred_hours_together",
      "version": 1,
      "enabled": true,
      "category": "voice",
      "trigger": "voice",
      "rarity": "rare",
      "importance": 70,
      "titles": {
        "zh-CN": "百小时的声音",
        "en-US": "One Hundred Hours"
      },
      "descriptions": {
        "zh-CN": "这个房间累计留下了一百小时的声音。",
        "en-US": "One hundred hours of voices were left in this room."
      },
      "condition": {
        "metric": "active_voice_seconds",
        "operator": "crosses",
        "value": 360000
      },
      "dedupe": {
        "scope": "room",
        "mode": "forever"
      }
    }
  ]
}
```

事件触发时会保存规则版本和多语言文案快照。以后修改规则不会改变已经产生的历史记忆。

`match_mode` 默认为 `transition`：只有整体条件由不满足变为满足才生成事件，防止新规则对已达标历史数据补发。设为 `state` 则允许下一次对应触发器到来时按当前达标状态补发（仍遵守去重/冷却）。不会伪造历史发生时间，也不会重放过去的原始活动。`crosses` 本身始终要求跨过阈值；需要历史补发时使用 `gte` 配合 `state`。

## 条件

支持的指标：

- `active_voice_seconds`
- `room_age_days`
- `active_days`
- `consecutive_active_days`
- `distinct_members`
- `participant_count`
- `concurrent_speakers`
- `session_count`
- `session_duration_seconds`
- `silence_seconds`
- `last_two_duration_seconds`
- `last_one_duration_seconds`
- `member_voice_sessions`
- `pair_session_count`
- `text_message_count`
- `local_hour`

支持的操作符：`eq`、`gt`、`gte`、`lt`、`lte`、`between`、`crosses`、`changed`、`in`。

`between` 为闭区间；`local_hour` 使用服务端时区。`concurrent_speakers` 是当前近期说话人数，不是历史峰值。

`crosses` 只在指标从阈值以下跨越到阈值以上时命中，适合累计成就，避免新规则对所有历史房间立即补发。

使用 `all` 或 `any` 组合条件：

```json
{
  "all": [
    {"metric": "participant_count", "operator": "eq", "value": 2},
    {"metric": "last_two_duration_seconds", "operator": "gte", "value": 7200},
    {"metric": "local_hour", "operator": "between", "values": [0, 5]}
  ]
}
```

条件最多 16 个、最多嵌套 3 层；不支持脚本、正则或任意表达式。

最多 500 条自定义规则，规则文件上限 4 MiB，最多 8 种语言，每条文案不超过 512 字节。只允许白名单指标和操作符，未知 JSON 字段会被拒绝。引擎在加载时编译、按触发器分组，运行事件时不解析 JSON。

档案响应会保留最近可容纳的记录并设置 `has_more`，不超过控制通道 64KB 帧上限；聚合用的历史成员/配对明细不下发。完整事件和文案仍保存在服务端，当前客户端会提示仅展示最近部分记录。

## 触发器与去重

触发器：`created`、`join`、`voice`、`text`、`leave`、`session_end`、`tick`。

去重范围：

- `room`：每个房间独立；
- `member`：每个触发成员独立；
- `pair`：每一对当时在场成员独立。

成员范围在 `join/voice/text/leave` 使用触发成员，在 `tick` 使用当前成员，在 `session_end` 使用刚结束会话的成员。双人范围枚举对应成员集合中的每一对，分别记录触发者；`participants` 仍保留整个现场。会话结束在最后一人离开并超过重连宽限期后结算。

去重模式：

- `forever`：对应范围只触发一次；
- `cooldown`：经过 `cooldown_seconds` 后可以再次触发。

## 管理端接入

服务端 Archive Collector 提供传输无关的管理方法：

- `DeclarativeRules`
- `ReplaceDeclarativeRules`
- `UpsertDeclarativeRule`
- `DeleteDeclarativeRule`
- `ReloadDeclarativeRules`
- `DeclarativeRuleError`

Web 管理端推荐调用带管理员身份和乐观版本锁的服务方法：

- `SaveRuleDraft` / `DeleteRuleDraft`：保存、删除草稿，不影响运行中规则。
- `PublishRuleDraft`：校验并发布草稿。
- `SetRuleEnabled` / `RemoveRule`：停用、删除已发布规则。
- `CopyRule`：复制到新 ID 的草稿。
- `RollbackRule`：将历史内容作为更高版本重新发布，保留原去重 ID。
- `PreviewRule`：只读预览历史累计指标可能命中的房间，不产生事件、不消耗去重键。需要实时上下文的条件会明确标记，不能声称是历史回放。
- `BuiltinRuleCatalog`：只读列出 Go 内置规则，禁止同名覆盖。

每次管理写入需提供当前 `revision`；冲突返回 `ErrRuleRevisionConflict`。发布、回滚、删除及草稿操作记录管理员身份、时间和动作。文件保存最近 100 条历史规则版本和 1000 条管理审计。历史文案已独立保存在事件中，不受规则版本保留数量影响。

编辑已发布规则时必须递增该规则的 `version`，去重 ID 不随版本改变。外部文件热加载会校验版本、规范化并原子保存文档，记录 `local-file` 审计；因此该路径需要服务端读写权限。`local-file` 仅代表本地文件操作，不代表已认证的某位管理员。

存储通过 `RuleRepository` 接口注入，默认实现为 JSON 原子文件替换；未来可替换数据库。当前没有新增公开 HTTP 端口或 Web 页面：后续管理端需先完成鉴权、权限检查，再调用服务层。直接编辑规则文件属于具有服务端文件权限的运维操作，不替代管理端身份审计。

未来 Web 管理端应在鉴权和审计之后调用这些方法，不要直接构造运行时 Predicate。发布流程应先校验完整规则集，再原子持久化并替换内存规则；发布失败时当前规则保持不变。

## 语音优先与性能边界

Archive 不进入音频实时回调或服务端 UDP 转发链路。客户端说话状态最多每秒上报一次，客户端控制队列和服务端统计队列使用非阻塞入队；拥塞时可丢统计采样，不等待统计处理完成。这不代表零开销：控制消息、后台计算、内存分配、GC 和磁盘操作仍会竞争机器资源。

启动时若档案或规则文件损坏、不可访问，会记录警告并在本进程禁用 Archive，不阻止核心语音服务启动，也不会覆盖损坏文件。修复文件后重启服务端即可重新启用。运行中的非法规则热更新仍保留上一份有效规则。

档案正文通过独立的 `room-archive` 界面事件按需推送，不再包含在约 33 毫秒一次的常规状态快照中。响应在桥接层只消费一次；关闭档案页面会注销监听、清理待显示数据，关闭期间到达的响应会被忽略。服务端空集合发送 `{}` / `[]`，客户端同时兼容旧版本的 `null`。

统计处理只为本次会修改的映射保留前值，不再在每个语音采样、定时检查前后复制全部历史组合。内置成员/组合会话阈值使用会话结束时维护的摘要，启动时从已有历史重建；不改变存储格式。Go Predicate、Contexts、Actors、Key、Facts 回调必须同步、只读，不得修改或持有 EvalContext 中的映射和切片。需要长期保存的数据必须自行复制。

每秒定时检查保留，避免漏掉自定义时间规则。无状态变更时跳过整个持久化步骤，包括全量复制、JSON 序列化和磁盘写入；周年事件、会话结算和查询中生成的事件也会标记变更。持久化仍每 2 秒尝试一次，失败保持待保存状态并重试，正常关闭会排空队列后保存。这个间隔不是严格的最大丢失窗口：后台阻塞、写入失败或进程异常退出仍可能丢失尚未保存的数据。

仍需注意：有变更时仍是全量文件保存；单个语音采样仍需复制该房间历史成员表；双人范围规则仍枚举成员对，成本随现场人数平方增长。新增复杂 Go Predicate 应避免 IO、无界遍历和长时间计算。硬关闭会禁用 Collector 和能力通告；配置生效需重启服务端，客户端重新连接后停止上报。

可在 `server` 目录运行 `go test ./internal/archive -run '^$' -bench 'Benchmark(ProcessorHistory|CleanFlush|Declarative500Rules)$' -benchmem`。这些是统计微基准，不是端到端通话延迟或丢包测试，不能直接把其加速比例解释为语音性能提升。
