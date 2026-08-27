import { defineConfig } from 'vitepress'

export default defineConfig({
  title: "cbld",
  description: "A modern build engine for C/C++.",
  appearance: 'dark',
  head: [['link', { rel: 'icon', href: '/favicon.svg' }]],
  themeConfig: {
    logo: '/logo.svg',
    nav: [
      { text: 'Home', link: '/' },
      { text: 'GitHub', link: 'https://github.com/iamvxrn/cbld' }
    ],
    search: {
      provider: 'local'
    },
    sidebar: [
      {
        text: 'START',
        items: [
          { text: 'Overview', link: '/overview' },
          { text: 'Install', link: '/install' },
          { text: 'Quickstart', link: '/quickstart' },
          { text: 'Packages', link: '/packages' },
        ]
      },
      {
        text: 'REFERENCE',
        items: [
          { text: 'Manifest', link: '/manifest' },
          { text: 'CLI Reference', link: '/cli' },
          { text: 'Architecture', link: '/architecture' },
        ]
      }
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/iamvxrn/cbld' }
    ]
  }
})
