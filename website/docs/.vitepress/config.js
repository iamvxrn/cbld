import { defineConfig } from 'vitepress'

export default defineConfig({
  title: "cbld",
  description: "A modern build engine for C/C++.",
  appearance: 'dark',
  // Static files in docs/public/ (`cbld-libs`, `cbld-libs.toml`) are served at
  // the site root, but VitePress's markdown linker only knows about pages.
  ignoreDeadLinks: ['/cbld-libs', '/cbld-libs.toml'],
  head: [
    ['link', { rel: 'icon', href: '/favicon.svg' }],
    ['meta', { property: 'og:image', content: 'https://cbld.pages.dev/social.png' }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: 'https://cbld.pages.dev/social.png' }],
  ],
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
          { text: 'Changelog', link: '/changelog' },
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
