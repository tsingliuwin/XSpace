---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: XSpace
  text: 极速磁盘空间探测器
  tagline: 基于 Rust 的跨平台磁盘分析与空间清理辅助工具，多线程并行引擎，数秒深度检索全盘。
  image:
    src: /logo.png
    alt: XSpace
  actions:
    - theme: brand
      text: 立即下载
      link: /download/
    - theme: alt
      text: 使用指南
      link: /guide/

features:
  - icon: ⚡
    title: 极速并发扫描
    details: 多线程并行遍历，目录层级并行预读。单遍聚合计算目录子树大小与文件数，不再对每个匹配目录重走子树。
    link: /features#性能设计
    linkText: 了解架构
  - icon: 🎯
    title: 五种扫描模式
    details: 大文件 / 空文件 / 空目录 / 按名搜文件 / 按名搜目录。正交维度驱动，配置灵活组合。
    link: /features#扫描模式一览
    linkText: 查看模式
  - icon: 📊
    title: 目录聚合分析
    details: 搜目录时按"大小 / 文件数 / 两者"聚合，结果在该 root 扫描结束后批量呈现，并按所选维度排序。
    link: /guide#扫描模式
    linkText: 使用方法
  - icon: 🗂️
    title: 结果复制与导出
    details: 一键复制单行或全部路径到剪贴板，或把完整结果清单导出为 TXT（Tab 分隔，可粘贴进 Excel）。
    link: /guide#结果操作-复制-导出-定位
    linkText: 查看操作
  - icon: 💾
    title: 多分区智能选择
    details: 勾选扫描分区，自动去重嵌套目录；跳过虚拟系统目录与云同步（iCloud/OneDrive 等）占位目录，避免卡死。
  - icon: 🪶
    title: 轻量原生
    details: 纯 Rust + egui，单文件约 5MB，零额外运行时依赖，macOS / Windows / Linux 原生运行。
---

<div class="home-screenshot">
  <img src="/xspace.png" alt="XSpace 界面预览" />
  <div class="caption">XSpace 主界面 · 极速磁盘空间探测器</div>
</div>
