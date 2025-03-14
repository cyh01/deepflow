![DeepFlow](./docs/deepflow-logo.png)
=====================================

简体中文 | [English](./README.md)

# 什么是 DeepFlow

DeepFlow 是[云杉网络](https://yunshan.net)开源的一款**高度自动化**的可观测性平台，是为云原生应用开发者建设可观测性能力而量身打造的全栈、全链路、高性能数据引擎。DeepFlow 使用 eBPF、WASM、OpenTelemetry 等新技术，创新的实现了 AutoTracing、AutoMetrics、AutoTagging、SmartEncoding 等核心机制，帮助开发者提升埋点插码的自动化水平，降低可观测性平台的运维复杂度。利用 DeepFlow 的可编程能力和开放接口，开发者可以快速将其融入到自己的可观测性技术栈中。

# 六大主要特性

- **全栈**：DeepFlow 使用 AF\_PACKET、BPF、eBPF 技术实现的 **AutoMetrics** 机制，可以自动采集任何应用的 RED（Request、Error、Delay）性能指标，精细至每一次应用调用，覆盖从应用到基础设施的所有软件技术栈。在云原生环境中，DeepFlow 的 **AutoTagging** 机制自动发现服务、实例、API 的属性信息，自动为每个观测数据注入丰富的标签，从而消除数据孤岛，并释放数据的下钻能力。
- **全链路**：DeepFlow 使用 eBPF 技术创新的实现了 **AutoTracing** 机制，在云原生环境中自动追踪任意微服务、基础设施服务的分布式调用链。在此基础上，通过与 OpenTelemetry 的数据集成，DeepFlow 将 eBPF Event 与 OTel Span 自动关联，实现完整的全栈、全链路追踪，让追踪无盲点。
- **高性能**：DeepFlow 创新的 **SmartEncoding** 标签注入机制，能够将标签数据的存储性能提升 10 倍，从此告别高基标签和数据采样焦虑。DeepFlow 使用 Rust 实现 Agent，拥有极致处理性能的同时保证内存安全。DeepFlow 使用 Golang 实现 Server，重写了 Golang 的 map、pool 基础库，数据查询和内存申请均有近 10 倍的性能提升。
- **可编程**：DeepFlow 目前支持了对 HTTP 1/2/S、Dubbo、MySQL、Redis、Kafka、MQTT、DNS 协议的解析，并将保持迭代增加更多的应用协议支持。除此之外，DeepFlow 基于 WASM 技术提供了可编程接口，让开发者可以快速具备对私有协议的解析能力，并可用于构建特定场景的业务分析能力，例如 5GC 信令分析、金融交易分析、车机通信分析等。
- **开放接口**：DeepFlow 拥抱开源社区，支持接收广泛的可观测数据源，并利用 AutoTagging 和 SmartEncoding 提供高性能、统一的标签注入能力。DeepFlow 支持插件式的数据库接口，开发者可自由增加和替换最合适的数据库。DeepFlow 向上为所有观测数据提供统一的标准 SQL 查询能力，便于使用者快速集成到自己的可观测性平台中，也提供了在此基础上继续开发方言 QL 的可能性。
- **易于维护**：DeepFlow 的内核仅由 Agent、Server 两个组件构成，将复杂度隐藏在进程内部，将维护难度降低至极致。DeepFlow Server 集群可对多资源池、异构资源池、跨 Region/AZ 资源池中的 Agent 进行统一管理，且无需依赖任何外部组件即可实现水平扩展与负载均衡。

# 文档

详细信息请访问[文档站点](https://deepflow.yunshan.net/docs/zh/?from=github)。

# 快速上手

DeepFlow 共有三种版本：
- DeepFlow Community：DeepFlow 社区版，面向开发人员
- DeepFlow Enterprise：DeepFlow 企业版，面向组织、解决团队协作的问题
- DeepFlow Cloud：DeepFlow SaaS 服务，目前处于测试阶段

DeepFlow 社区版由企业版的核心组件构成。通过开源，我们希望让观测更自动，让全世界的开发者更自由。

## 部署 DeepFlow Community

请参考[文档](https://deepflow.yunshan.net/docs/zh/install/all-in-one/?from=github)部署 DeepFlow Community。

同时我们也搭建了一个完整的 [DeepFlow Community Demo](https://ce-demo.deepflow.yunshan.net/?from=github)，欢迎体验。登录账号 / 密码：deepflow / deepflow。

## 体验 DeepFlow Cloud

[DeepFlow Cloud](https://deepflow.yunshan.net/) 是 DeepFlow 的全托管 SaaS 服务，目前处于测试阶段，仅支持中文。

## 体验 DeepFlow Enterprise

[DeepFlow Enterprise](https://www.yunshan.net/products/deepflow.html) 支持对混合云的全栈、全链路监控，覆盖容器、云服务器、宿主机、NFV网关，目前仅支持中文，欢迎联系我们进行体验。

# 从源码编译 DeepFlow

- [编译 deepflow-agent](./agent/build_cn.md)

# 软件架构

DeepFlow Community 主要由 Agent 和 Server 两个进程组成。每个 K8s 容器节点、虚拟机或物理裸机中运行一个 Agent ，负责该服务器上所有应用进程的 AutoMetrics 和 AutoTracing 数据采集。Server 运行在一个 K8s 集群中，提供 Agent 管理、数据标签注入、数据写入、数据查询服务。

![DeepFlow 软件架构](./docs/deepflow-architecture.png)

# 里程碑

这里有我们[未来的 Feature 规划](https://deepflow.yunshan.net/docs/zh/about/milestone/?from=github)。欢迎 Issue 和 Pull Request。

# 加入技术交流群

DeepFlow 官方社群对外开放，欢迎加入讨论：

<img src=./docs/wechat-group-keeper.png width=30% />

# 致谢

- 感谢 [eBPF](https://ebpf.io/)，革命性的 Linux 内核技术
- 感谢 [OpenTelemetry](https://opentelemetry.io/)，提供了采集应用可观测性数据的标准 API
