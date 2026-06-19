import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  lang: 'zh-CN',
  title: 'XSpace',
  description: '极速磁盘空间探测器 — 基于 Rust 的跨平台磁盘分析与空间清理辅助工具',
  lastUpdated: true,
  cleanUrls: true,

  head: [
    ['link', { rel: 'icon', type: 'image/png', href: '/favicon.ico' }],
    ['meta', { name: 'theme-color', content: '#2563EB' }],
    ['meta', { property: 'og:url', content: 'https://xspace.xi-n.com' }],
    ['meta', { property: 'og:title', content: 'XSpace · 极速磁盘空间探测器' }],
    ['meta', { property: 'og:description', content: '基于 Rust 的跨平台磁盘分析与空间清理辅助工具' }],
    ['meta', { property: 'og:image', content: 'https://xspace.xi-n.com/logo.png' }],
    ['meta', { property: 'og:type', content: 'website' }]
  ],

  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    siteTitle: 'XSpace',
    logo: '/logo.png',

    nav: [
      { text: '首页', link: '/' },
      { text: '功能特性', link: '/features' },
      { text: '使用指南', link: '/guide' },
      { text: '下载', link: '/download' }
    ],

    sidebar: {
      '/guide/': [
        {
          text: '使用指南',
          items: [
            { text: '快速开始', link: '/guide/' },
            { text: '扫描模式', link: '/guide#扫描模式' },
            { text: '结果操作', link: '/guide#结果操作-复制-导出-定位' }
          ]
        }
      ],
      '/features/': [
        {
          text: '功能特性',
          items: [
            { text: '核心能力', link: '/features' },
            { text: '扫描模式一览', link: '/features#扫描模式一览' },
            { text: '性能设计', link: '/features#性能设计' }
          ]
        }
      ]
    },

    socialLinks: [
      // 如有公开仓库可在此添加 { icon: 'github', link: 'https://github.com/...' }
    ],

    search: {
      provider: 'local'
    },

    outline: {
      label: '本页导航',
      level: [2, 3]
    },

    docFooter: {
      prev: '上一页',
      next: '下一页'
    },

    lastUpdatedText: '最后更新',

    footer: {
      message: '基于 Rust + egui 构建',
      copyright: '© 苏州遇码科技有限公司'
    }
  }
})
