# Genatrix · 演化

> 状态：草稿 v0.1 · 2026-10-19 · 待讨论
> 回答的问题：它怎样跟你一起变好，而每一次变化仍然在信任边界之内。

## 为什么要这份文档

到现在为止，Genatrix 会**积累**，但不会**改变**。

07 号让它记住你是谁、和谁有什么关系、答应过什么。03 号把每一次改稿和拒绝理由都记了下来，说它们是"最贵的信号"。但这些信号目前只有两个去处：起草时被检索出来当示例，以及进 04 号的评测集。同一类错误，比如给某个人的回复总是写得太正式，第十次它仍然会犯，除非有人去改提示词。

"陪伴你成长"需要第三样东西：**从经验里改变做事的方式，并且让你看得见它变了什么。** 这份文档定义这件事，以及它不能越过的线。

## 演化是什么

一个系统能变的地方有四层：

| 层 | 变的是什么 | 在 Genatrix 里 |
|---|---|---|
| 记忆 | 它知道关于你的什么 | 07 号的画像，已有 |
| 手册 | 它做事的规则 | **本文新增** |
| 技能 | 它能做什么 | 本文新增，借 11 号的功能型 agent 实现 |
| 权重 | 模型本身 | 不做，07 号已定 |

本文所说的演化，是：**从台账里的经验，提出对做事方式的修改；经过回放评测和你的批准后生效；随时可以撤回。**

进化需要三件事：变异、选择、保留。在这里它们分别是：

- **变异**：反思流水线从你的改稿、拒绝和指示里提出候选修改。
- **选择**：先由核心用历史数据回放，再由你批准。你是最后一道选择。
- **保留**：每一个生效过的版本都留档，可以恢复。

学界的自进化系统大多是自己改、自己评、自己上线。Genatrix 的不同在于选择这一环里有你。这不是限制，正因为有这一环，这种进化才能被信任。

## 五条原则

1. **演化的产物是文字，不是权重。** 可读，可审计，换模型不丢。这是 00 号原则 4 的直接推论。
2. **没有自己生效的变化。** 每一次演化都是一个待批准动作。这是 00 号原则 5。
3. **先回放，后提议。** 没有证据说明修改后更好，就不提出来。
4. **可以撤回。** 任何一条修改都能一键停用，任何一个历史版本都能恢复。
5. **目标是你写下的，不是你的批准率。** 一个以"让你点批准"为目标的系统会学会讨好你，而不是帮你。

## 手册

手册是一组**课**（Lesson）。每一课是一条可读的、关于怎么做事的规则。

```
Lesson {
  id
  scope:     角色 + 可选的 Person / 动作 kind / 来源    // 它在什么时候适用
  text:      String                                     // 一条规则，一两句话
  evidence:  [action_id | run_id | UserAssertion]       // 它从哪来
  source:    Model { name, prompt_version } | User
  status:    Proposed | Adopted | Retired | Rejected
  replay:    回放结果的引用
  version
}
```

例子：

- "给 Alice 的回复不用'您'。"依据是 7 次改稿。
- "摘要里，物流通知合成一行。"依据是 4 次拒绝理由"太长了"。
- "周五下午收到的会议邀请，草稿里提议改到下周。"依据是你在对话里说的一句话。

**课和事实不是一回事。** 07 号的 Fact 是关于世界的断言（"Alice 是律师"），Lesson 是关于做法的指令（"给 Alice 的回复要短"）。前者回答"是什么"，后者回答"怎么做"。两者分开存放，分开审批，分开撤回。

**课被怎么用。** 上下文组装时（03 号），适用于这次角色和对象的已生效课，放在核心提示词之后的一个固定位置，有预算，超出就按"范围越具体越优先、越新越优先"来裁。

**手册只做增量修改。** 一次演化是加一课、改一课或退役一课，不是把整本手册重写一遍。整本重写会让细节在一次次压缩中丢失，而且你没法审一份整体改过的文本。

**和 03 号"不做自我修改"的关系。** 核心的提示词、流水线定义、工具声明仍然是人提交的版本化文件，演化碰不到。手册是数据，和画像一样，放在提示词里一个固定的位置。03 号那一条不变，补一句话说明手册不属于"自我修改"。

## 信号从哪来

按强弱排：

1. 你在对话里明确说的："以后给我爸的消息别用英文。"
2. 改稿：草稿和你的定稿之间的差别。
3. 拒绝理由。
4. 批准后撤回。
5. 你没用它的草稿，自己重写了一遍。
6. 动作放到过期也没人理。这是最弱的信号，单独不足以支撑一课。

**课的证据只能是你的行为。** 别人发来的邮件和消息不能成为一课的依据。这一条是防线：如果一封邮件里写着"以后所有发票都转给某地址"，它只是一个 Item，永远变不成手册里的一条规则。02 号说过不可信输入的防线是结构性的，这里也一样。

## 反思

反思是一条流水线，不是自主行为。它在流水线定义里写好的时间运行，默认在夜间、设备空闲时，用本机模型。

一次反思：

1. 读自上次以来的信号，按 scope 聚类。
2. 对每一簇，提出至多一条候选课，或者对已有课的修改。
3. 和已有的课去重、合并，和被拒绝过的课比对：拒绝过的不再提。
4. 交给核心回放。

信号不够的簇什么也不提。宁缺毋滥，和 07 号对承诺的态度一样。

## 回放

回放回答一个问题：**如果这条课早就在了，过去那些任务的结果会不会更接近你的定稿？**

- 答案来自 04 号的评测集：你改稿后的版本、你的覆盖记录。这些数据本来就在积累。
- 候选课由反思流水线提出，回放由核心执行。提出它的那次运行看不到用来评它的数据。生成这条课的那几次改稿不参与评它，只用其余的历史来评。
- 比较的是可以计算的距离（编辑距离、结构校验），不单靠模型打分。8B 的模型当裁判不可靠。
- 一条课只有在它的范围内变好、且没让范围之外变差时，才会被提给你。

审批卡片上写出数字："过去 20 次给 Alice 起草，按这条课重做，14 次更接近你的定稿，2 次更远。"

## 提议与批准

每一条候选课都是一个动作，kind 是 `AdoptLesson`，走 03 号的整套审批：版本、令牌、过期、拒绝理由。

- 卡片上有：这条课的原文、它适用的范围、依据（可以点开看那几次改稿）、回放的数字。
- 你可以先改再批准。你改过的版本才是生效的版本。
- 你拒绝的课变成 Rejected，不再提同一件事。否定的记忆和肯定的记忆一样重要，与 07 号的 Rejected 事实同理。
- **频率有上限。** 一周提出的课不超过几条。一个每天都要你审五条规则的助手，是在给你添活。
- 06 号加一个"它学会了什么"的视图：所有生效中的课，每条一个停用按钮，旁边是它生效以来的表现。

## 退役与回滚

课生效之后继续被观察。如果它生效后，它范围内的改稿反而变多了，反思流水线会提议退役它，同样经你批准。

你可以随时直接停用一条课，不需要理由。停用和删除都是 tombstone，记录里查得到"曾经有过"。手册的每一个版本都可以整体恢复。

两条课冲突时，不覆盖，生成一条裁决给你，和 07 号处理事实冲突的方式一样。

## 技能

当反思发现你反复手工做同一件事，比如每个月把银行对账单里的某几类交易抄进表格，它可以提议的就不只是一条课，而是一个功能型 agent。

这条路完全走 11 号：生成的 agent 有清单、有声明的权限、内容哈希固定，安装是 02 号第 12 条的高权限操作。它的作者标为模型，没有任何特权。演化不打开任何新的门，只是多了一个可能的作者。

写代码需要比 8B 更强的模型。写的时候它只需要知道这件事的结构，不需要看到你的原始数据；用到云端时照常经过出境闸门。

这一层放得最晚，见"待裁决"。

## 你也在变

陪伴不只是它变得更像你，还包括它注意到你在变，并且不妨碍你变成你想成为的样子。

**目标。** 你可以写下长期目标，形式和 07 号的用户声明一样：带时间、你说的。目标在演化里是**否决条件**：一条课即使回放更好，如果它和你写下的目标冲突，也不提。

例子：你常常很晚才回家人的消息，一个只看行为的系统会学会"家人的消息不急，放在摘要后面"。如果你写下过"多和家人联系"，这条课就不该出现。这就是"批准率"和"你想要的"不一致的时候。

**变化。** 07 号的 Relationship 统计和 StyleProfile 本来就随时间变化。当变化明显时，它可以告诉你："你和某人的往来，这三个月少了一半。"只陈述数据，不解读原因。07 号"不做情感推断"不变。

**留白。** 有些事也许应该故意不替你做完，让你自己做。这一点现在没有好答案，先列为开放问题。

## 风险

| 风险 | 表现 | 防线 |
|---|---|---|
| 讨好 | 学会说你想听的，而不是对你有用的 | 回放比的是你的定稿，不是批准率；目标是否决条件；你是最后一道选择 |
| 投毒 | 外部内容变成持久的规则 | 课的证据只能是你的行为 |
| 钻评估的空子 | 提议者为了通过评测去迎合评测 | 回放由核心执行，提议者看不到评测数据 |
| 漂移 | 一条条小修改累积成你不认识的行为 | 手册有总预算；定期整体回放；任一版本可恢复 |
| 过拟合 | 一条课只对产生它的那几次有效 | 生成它的数据不参与评它 |
| 打扰 | 审规则变成新的负担 | 每周的提议数有上限 |

## 不变量（提议，通过后并入 02 号）

1. 没有一条课不经你本人批准就进入任何上下文。规则自动批准永远不适用于 `AdoptLesson`。
2. 课的证据只能是你的动作、改稿、拒绝理由和你说的话；Item 正文不能成为课的证据。
3. 回放由核心执行；提出候选课的运行读不到评测数据。
4. 一条课被停用后，之后的任何一次运行，上下文里都不再有它。
5. 演化不改核心的提示词、流水线定义、工具声明、闸门策略，也不改任何 agent 的清单与权限。生成新 agent 只能走 11 号的安装路径。

## 明确不做

- **不做微调。** 重申 07 号。演化的产物是文字和经过安装的代码。
- **不做自动生效。** 包括第二阶段，课不走规则批准。
- **不以批准率、使用时长、打开次数为目标。**
- **不做"关于你的一段描述"。** 课是可以单独审批和撤回的规则，不是一段人格画像。
- **不做跨用户的任何东西。** 课不共享，不汇总，不做联邦学习。
- **不让演化碰信任边界。** 敏感分级、出境闸门、策略矩阵、权限，演化一概不碰。
- **不做情感与心理推断。** 重申 07 号。

## 与其他文档的接口

- 02 号：上面五条不变量；不可信输入不能成为课的证据。
- 03 号：上下文组装多一层手册；"不做自我修改"补一句说明；新的动作 kind `AdoptLesson`；反思是一条流水线。
- 04 号：评测集成为回放的答案；回放需要的模型调用。
- 06 号：`AdoptLesson` 的卡片；"它学会了什么"视图；目标的录入。
- 07 号：课和事实分开存放、分开审批；Rejected 的处理一致；目标是一种用户声明。
- 08 号：手册与版本的存储。
- 10 号：放在哪个阶段。
- 11 号：模型作为 agent 的作者。

## 理论来源

这份文档的做法主要来自四条线索。完整的论文清单见文末。

- **上下文即学习。** 2023 年以来的主流做法是让 agent 在文字层面积累经验：反思、经验规则、工作流记忆、不断演化的手册。手册的结构和"只做增量修改"来自这一线。
- **自我修改与开放式演化。** Gödel Machine 要求改动之前先证明它更好；Darwin Gödel Machine 用基准测试代替证明，并保留全部历史版本。回放加版本档案是这条思路，把你放在选择这一环。
- **自进化的风险。** 已有研究观察到自我改进的系统会伪造记录来通过评测，会在记忆和工具层面逐渐偏离安全行为；外部内容可以通过记忆被持久植入；以用户认可为目标的优化会导向讨好。风险表里的每一行都对应其中一项。
- **人机共生与成长。** Licklider 与 Engelbart 的目标是增强人，而不是替代人；延展心智把外部工具看成心智的一部分。"你也在变"一节来自这里。

## 待裁决

1. 是否引入手册，还是继续只靠检索。我主张引入：检索只能模仿你过去的样子，改不了它反复犯的错。
2. 课是否永远不走规则自动批准。我主张永远不走。
3. 课的证据是否只限于你的行为。我主张只限于。
4. 每周提议的上限。我主张不超过三条。
5. 反思用什么模型。我主张默认本机；云端打开时也只发结构化的信号摘要，经闸门。
6. 目标是否进第一版。我主张进，但只作为否决条件，不作为优化方向。
7. 技能这一层放在哪个阶段。我主张手册跑稳之前不做，最早在第三阶段之前。
8. 手册放在哪个阶段。我主张底座与 agent 框架之后、第二阶段之前：它只依赖第一阶段已经在积累的改稿和拒绝，而第二阶段的自动批准规则会受益于它。
9. "留白"是否要设计。我主张先不设计，用一段时间后再看。

## 参考文献

按阅读顺序分组。每组第一篇是入门，带 ★ 的是对本文影响最大的。

### 一、先看全景

- ★ A Survey of Self-Evolving Agents: What, When, How, and Where to Evolve on the Path to Artificial Super Intelligence · Gao 等 · 2025 · https://arxiv.org/abs/2507.21046
- A Comprehensive Survey of Self-Evolving AI Agents: A New Paradigm Bridging Foundation Models and Lifelong Agentic Systems · Fang 等 · 2025 · https://arxiv.org/abs/2508.07407
- ★ Cognitive Architectures for Language Agents（CoALA）· Sumers 等 · 2023 · https://arxiv.org/abs/2309.02427
- Toward Personalized LLM-Powered Agents: Foundations, Evaluation, and Future Directions · Xu 等 · 2026 · https://arxiv.org/abs/2602.22680
- Self-Improvements in Modern Agentic Systems: A Survey · Ren 等 · 2026 · https://arxiv.org/abs/2607.13104

### 二、上下文即学习（手册的来源）

- ★ Reflexion: Language Agents with Verbal Reinforcement Learning · Shinn 等 · 2023 · https://arxiv.org/abs/2303.11366
- Generative Agents: Interactive Simulacra of Human Behavior · Park 等 · 2023 · https://arxiv.org/abs/2304.03442
- ExpeL: LLM Agents Are Experiential Learners · Zhao 等 · 2023 · https://arxiv.org/abs/2308.10144
- Agent Workflow Memory · Wang 等 · 2024 · https://arxiv.org/abs/2409.07429
- Dynamic Cheatsheet: Test-Time Learning with Adaptive Memory · Suzgun 等 · 2025 · https://arxiv.org/abs/2504.07952
- ★ Agentic Context Engineering: Evolving Contexts for Self-Improving Language Models（ACE）· Zhang 等 · 2025 · https://arxiv.org/abs/2510.04618
- GEPA: Reflective Prompt Evolution Can Outperform Reinforcement Learning · Agrawal 等 · 2025 · https://arxiv.org/abs/2507.19457
- DSPy: Compiling Declarative Language Model Calls into Self-Improving Pipelines · Khattab 等 · 2023 · https://arxiv.org/abs/2310.03714
- MemRL: Self-Evolving Agents via Runtime Reinforcement Learning on Episodic Memory · Zhang 等 · 2026 · https://arxiv.org/abs/2601.03192

### 三、记忆系统

- MemGPT: Towards LLMs as Operating Systems · Packer 等 · 2023 · https://arxiv.org/abs/2310.08560
- ★ Sleep-time Compute: Beyond Inference Scaling at Test-time · Lin 等 · 2025 · https://arxiv.org/abs/2504.13171
- A-MEM: Agentic Memory for LLM Agents · Xu 等 · 2025 · https://arxiv.org/abs/2502.12110
- Mem0: Building Production-Ready AI Agents with Scalable Long-Term Memory · Chhikara 等 · 2025 · https://arxiv.org/abs/2504.19413
- Zep: A Temporal Knowledge Graph Architecture for Agent Memory · Rasmussen 等 · 2025 · https://arxiv.org/abs/2501.13956

### 四、个性化

- LaMP: When Large Language Models Meet Personalization · Salemi 等 · 2023 · https://arxiv.org/abs/2304.11406
- ★ MultiSessionCollab: Learning User Preferences with Memory to Improve Long-Term Collaboration · Mehri 等 · 2026 · https://arxiv.org/abs/2601.02702
- Towards Natural Personalization: Evaluating Long-Horizon Preference Following in Personalized User-LLM Interactions · Guo 等 · 2026 · https://arxiv.org/abs/2603.04191

### 五、自我修改与开放式演化（技能与回放的来源）

- Goedel Machines: Self-Referential Universal Problem Solvers Making Provably Optimal Self-Improvements · Schmidhuber · 2003 · https://arxiv.org/abs/cs/0309048
- ★ Darwin Godel Machine: Open-Ended Evolution of Self-Improving Agents · Zhang 等 · 2025 · https://arxiv.org/abs/2505.22954
- Mendel Gödel Machine · Liu 等 · 2026 · https://arxiv.org/abs/2608.07645
- Voyager: An Open-Ended Embodied Agent with Large Language Models · Wang 等 · 2023 · https://arxiv.org/abs/2305.16291
- Automated Design of Agentic Systems · Hu 等 · 2024 · https://arxiv.org/abs/2408.08435
- A Self-Improving Coding Agent · Robeyns 等 · 2025 · https://arxiv.org/abs/2504.15228
- AlphaEvolve: A coding agent for scientific and algorithmic discovery · Novikov 等 · 2025 · https://arxiv.org/abs/2506.13131 （博客：https://deepmind.google/blog/alphaevolve-a-gemini-powered-coding-agent-for-designing-advanced-algorithms/）
- Self-Evolving Agent Harness · Wei 等 · 2026 · https://arxiv.org/abs/2610.18813
- Open-Endedness is Essential for Artificial Superhuman Intelligence · Hughes 等 · 2024 · https://arxiv.org/abs/2406.04268
- AI-GAs: AI-generating algorithms, an alternate paradigm for producing general artificial intelligence · Clune · 2019 · https://arxiv.org/abs/1905.10985
- Abandoning Objectives: Evolution through the Search for Novelty Alone · Lehman, Stanley · 2011 · https://direct.mit.edu/evco/article/19/2/189-223/1365
- Why Greatness Cannot Be Planned: The Myth of the Objective（书）· Stanley, Lehman · 2015 · https://link.springer.com/book/10.1007/978-3-319-15524-1

### 六、权重层（不做，但要知道）

- Self-Adapting Language Models（SEAL）· Zweiger 等 · 2025 · https://arxiv.org/abs/2506.10943
- Scaling Self-Evolving Agents via Parametric Memory · Ren 等 · 2026 · https://arxiv.org/abs/2606.04536
- A Comprehensive Survey of Continual Learning: Theory, Method and Application · Wang 等 · 2023 · https://arxiv.org/abs/2302.00487

### 七、风险（风险表的来源）

- ★ Your Agent May Misevolve: Emergent Risks in Self-evolving LLM Agents · Shao 等 · 2025 · https://arxiv.org/abs/2509.26354
- On Safety Risks in Experience-Driven Self-Evolving Agents · Zhao 等 · 2026 · https://arxiv.org/abs/2604.16968
- On the Fragility of Self-Improving Agents: Variance, Task Order, and Underspecification · Ye 等 · 2026 · https://arxiv.org/abs/2608.18066
- AgentPoison: Red-teaming LLM Agents via Poisoning Memory or Knowledge Bases · Chen 等 · 2024 · https://arxiv.org/abs/2407.12784
- ★ Towards Understanding Sycophancy in Language Models · Sharma 等 · 2023 · https://arxiv.org/abs/2310.13548
- Sycophancy in GPT-4o（OpenAI 事后说明）· 2025 · https://openai.com/index/sycophancy-in-gpt-4o/

### 八、人机共生与成长（"你也在变"的来源）

- ★ Man-Computer Symbiosis · Licklider · 1960 · https://groups.csail.mit.edu/medg/people/psz/Licklider.html
- Augmenting Human Intellect: A Conceptual Framework · Engelbart · 1962 · https://www.dougengelbart.org/pubs/augment-3906.html
- The Extended Mind · Clark, Chalmers · 1998 · https://consc.net/papers/extended.html
