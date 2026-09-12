# Genatrix · 数据模型

> 状态：草稿 v0.1 · 2026-09-12 · 待品味把关
> 回答的问题：一切数字痕迹如何归一成一个 Item，同时不丢掉任何原始信息。

## 一个模型，四条理由

邮件、聊天、日程、笔记、文件，本来是五种东西。归一成一个 Item，不是为了简洁，是为了四件后面的事只做一遍：

- 一条时间线。你的一天不是按 App 分开发生的。
- 一套检索。搜一个人的名字，邮件和聊天一起出来。
- 一套权限与敏感分级。02 号文档只需要面对一种对象。
- 一种导出格式。00 号承诺的"可迁出"，落到一个 JSONL 加一个附件目录。

代价是某些种类的字段会空着。日程没有发件人，文件没有线程。这个代价接受，用"脊柱加载荷"的结构来承受。

## 三层，不混

```
Raw          原始记录。连接器拿到什么就存什么，字节级保真，永不修改。
Item         归一条目。从 Raw 派生，人读的形态。
Annotation   标注。AI 或规则从 Item 派生的一切：摘要、向量、实体、标签。
```

三层之间的方向是单向的：Raw 生成 Item，Item 生成 Annotation。反过来永远不发生。Item 里没有任何一个字段是 AI 写的。AI 的输出全部是 Annotation，挂在 Item 旁边，可以重算、可以删除、可以并存多个版本。

这是本文档最重要的一条边界。它保证两件事：归一逻辑有 bug 时，从 Raw 重跑就能修好；模型换代时，重算 Annotation 就能升级，你的数据本身一个字节不动。

## 实体

六个实体。每个都有 ULID 作内部 id，时间可排序，不暴露任何来源信息。

| 实体 | 中文 | 一句话 |
|---|---|---|
| Raw | 原始记录 | 连接器某次拉到的一个源对象，字节级保真 |
| Item | 条目 | 时间线上的一格：一封邮件、一条消息、一个日程、一篇笔记、一个文件 |
| Thread | 线程 | Item 归属的对话：邮件会话、聊天群、私聊、日历本、笔记本 |
| Person | 人 | 一个真实的人，跨来源有多个 Handle |
| Blob | 附件体 | 内容寻址的二进制，附件、图片、文件正文 |
| Annotation | 标注 | 从 Item 派生的机器产物，带来源模型与版本 |

### Raw

```
Raw {
  id
  source:      Source            // 哪个连接器、哪个账号、源里的 id
  fetched_at
  content_type                   // "message/rfc822"、"application/x-telegram-update" 等
  payload:     bytes             // 原样
  hash:        sha256(payload)
}
```

Raw 只增不改不删。同一个源对象被再次拉到，如果 hash 相同就跳过，不同就新增一条 Raw 并派生新版本的 Item。

### Source

所有实体溯源的锚点。

```
Source {
  connector:   "imap" | "telegram" | ...
  account:     账号标识（邮箱地址、Telegram 用户 id）
  external_id: 源系统内的唯一 id（Message-ID、Telegram message id）
}
```

三元组唯一。连接器重复拉取、断线续传、重装重导，全部靠它做到幂等。

### Item

脊柱加载荷。脊柱是所有种类共有的字段，载荷是每种自己的。

```
Item {
  id
  kind:         Mail | Message | Event | Note | File
  source:       Source
  raw_id                          // 派生自哪条 Raw
  supersedes:   Option<item_id>   // 上游被编辑，新版本指向旧版本
  thread_id
  occurred_at:  时间戳 + 时区      // 源里的发生时间，不是入库时间
  ingested_at
  direction:    Inbound | Outbound | Internal | Neutral
  author:       Option<person_id>
  recipients:   [person_id]       // 本条明确的收件人，不是线程全员
  text:         String            // 归一后的纯文本，检索与 AI 读的是它
  blobs:        [blob_id]
  sensitivity:  Level             // 见 02，默认 Sensitive
  tombstoned:   bool              // 上游已删除，见"上游删除"一节
  payload:      Payload           // 按 kind 不同
}
```

几个字段的理由：

- **direction 是"你"的视角。** Outbound 是你发的，Inbound 是发给你的，Internal 是你自己给自己的（备忘、Saved Messages），Neutral 是没有方向的东西（日程、文件）。这一个字段让"我上周答应过谁什么"这类问题变成一次过滤。
- **text 是归一纯文本，不是原文。** HTML 邮件剥掉标签，引用的上一封回复剥掉，Telegram 的格式实体展平。原文在 Raw 里。text 存在的唯一目的是让检索和模型读到干净的东西。
- **recipients 只放本条的收件人。** 群聊一千人，每条消息不复制一千个 id，成员表在 Thread 上。
- **sensitivity 默认 Sensitive。** 每一条新数据在被判定之前，都当作不能出设备。判定规则在 02。
- **supersedes 而不是原地改。** Telegram 允许编辑消息，笔记会改。每次变化是一个新 Item 指向旧的，时间线默认只显示最新版本，历史随时可看。

### Payload

```
Mail    { subject, from, to, cc, bcc, message_id, in_reply_to, references, headers: 精选 }
Message { reply_to: Option<item_id>, forwarded_from: Option<person_id>, edited: bool }
Event   { title, start, end, all_day, location, attendees: [person_id], status }
Note    { title, format: Markdown | Plain }
File    { name, mime, size }
```

载荷不做大而全。每种只放"没有它这一格就读不懂"的字段。想要更多，去 Raw。

### Thread

```
Thread {
  id
  kind:         MailThread | DirectChat | GroupChat | Channel | Calendar | Notebook | Folder
  source:       Source
  title:        Option<String>     // 邮件主题、群名、日历名
  members:      [person_id]        // 当前成员，群聊变动时更新
  first_at, last_at                // 由 Item 派生，做时间线排序用
}
```

Thread 是 Item 的容器，不是 Item 的父级。一封邮件的 Thread 由 References 头推出来，一条 Telegram 消息的 Thread 就是那个 chat。

### Person 与 Handle

```
Person {
  id
  display_name
  is_self:      bool               // 有且只有一个 Person 是你
  merged_from:  [person_id]        // 合并历史，可拆
}

Handle {
  id
  person_id
  kind:         Email | TelegramId | Phone | ...
  value
  confidence:   Confirmed | Inferred
}
```

这是 07 号"画像"的种子，也是数据模型里最需要克制的地方。

- 一个 Handle 第一次出现，自动建一个 Person。
- 同一个人的多个 Handle，合并靠两种方式：你手动确认，或者规则推断（同一 Telegram 联系人带手机号，同一邮件签名带电话）。推断的合并标 Inferred，界面上能一眼看出来，随时可拆。
- **AI 不做合并。** 合并是对"谁是谁"的断言，错了会污染画像，所以只能是规则或者你。AI 可以建议，建议是 Annotation。

### Blob

```
Blob {
  hash:         sha256             // 主键，内容寻址
  mime
  size
  name_hint:    Option<String>
}
```

同一个文件在十封邮件里出现，磁盘上只有一份。Blob 的正文在文件系统，数据库只存元数据。

### Annotation

```
Annotation {
  id
  item_id
  kind:         Summary | Embedding | Entities | Label | Sensitivity | Suggestion | ...
  producer:     { model, prompt_version } | { rule, version }
  created_at
  value:        按 kind 不同
  superseded_by: Option<annotation_id>
}
```

- 同一个 Item、同一个 kind 可以有多条 Annotation，producer 不同。模型换代后旧的不删，新的 supersede 旧的。
- **Sensitivity 判定本身也是 Annotation**，Item 上那个 sensitivity 字段是当前生效的判定结果的缓存。02 号会解释判定如何生效。
- Embedding 的 value 是向量加分块位置。分块策略属于 producer 的一部分。

## 时间

- `occurred_at` 是源里的时间，带时区。邮件取 Date 头，取不到用服务器收到时间；Telegram 取消息时间。
- `ingested_at` 是入库时间。两者差值大，说明是历史导入。
- 时间线按 `occurred_at` 排，永远不按 `ingested_at`。历史导入一百万条不会把今天的时间线冲掉。

## 上游删除与编辑

两个需要你裁决的取舍，我先写出我的主张。

**编辑：** 保留所有版本。用 `supersedes` 串起来。Telegram 一条消息改了三次，就是四个 Item，时间线显示最新，点开能看历史。这是你的记录，不是对方的。

**删除：** 上游删了，本机不删，标 `tombstoned`。默认时间线里隐藏，可以选择显示。理由是 00 号第一句话，数据归你。对方在 Telegram 里"对所有人删除"一条消息，它已经到过你的设备，就是你的记录。

这一条有伦理张力。它意味着 Genatrix 会保留别人希望撤回的内容。我认为这是正确的默认，因为替代方案是让对方决定你的记忆里有什么。但需要你确认，并且在 06 号界面文档里要给它一个诚实的呈现，不藏着。

## 身份与自我

`is_self` 的 Person 是整个模型的原点。direction 由它算出来，"我的承诺"、"待我回复"、"我发出去的"全部依赖它。

你的每个账号在接入时，把自己的 Handle 挂到这个 Person 上。这是接入流程的一部分，不是事后配置。

## 检索面

数据模型只规定什么可被检索，不规定引擎。

- 全文：`Item.text`、`Thread.title`、`Mail.subject`、`Person.display_name`、`Handle.value`。
- 向量：Embedding 类 Annotation。
- 结构：时间、方向、kind、Person、Thread、sensitivity、tombstoned。

任何一次检索的结果都是 Item 集合，再由 Thread 和 Person 展开上下文。

## 导出格式

00 号承诺可迁出，模型层面就定死：

```
export/
  items.jsonl         每行一个 Item，含 payload
  threads.jsonl
  persons.jsonl       含 handles
  annotations.jsonl   可选，不导出也能完整重建
  raw/                按 hash 存放原始记录
  blobs/              按 hash 存放附件
```

不导出数据库文件，导出的是模型。换一个实现能读回去，就算通过。

## 明确不做

- **不做结构化数据协议里的 owner、DID、lifetime。** 单用户单设备，owner 是隐含的。data-protocol 那套留给多方交互的未来。
- **不建 reaction、已读回执、正在输入之类的事件。** 第一阶段忽略。以后需要，作为 Item 的子事件加，不进 Item 本体。
- **不做通用 schema 扩展机制。** 新的 kind 就是新的 Payload 变体和一次迁移。宁可迁移十次，不要一个 JSON 万能字段。
- **不在模型里放 UI 状态。** 已读、置顶、归档是界面偏好，另存，导出时不带。

## 与后续文档的接口

- 02 号：`sensitivity` 的取值、判定与生效规则。
- 03 号：Action 实体，AI 提出的待批准动作。它引用 Item 但不属于数据模型，因为它是行为记录，不是数字痕迹。
- 05 号：每个连接器如何把源对象映射到 Raw 和 Item。
- 07 号：Person 之上的关系与画像。
- 08 号：这些实体如何落到 SQLite，Blob 如何加密落盘。

## 需要你裁决的

1. 上游删除保留并 tombstone，还是跟随删除。我主张保留。
2. 编辑保留全部版本。我主张保留。
3. Person 合并 AI 只能建议不能执行。我主张只建议。
4. 五个 kind 够不够第一阶段。我认为 Mail 和 Message 够，Event、Note、File 先定义不实现。
