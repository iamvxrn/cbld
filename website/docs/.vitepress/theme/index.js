import { h } from 'vue'
import DefaultTheme from 'vitepress/theme'
import './custom.css'

// Archived notice, shown above the nav on every page. VitePress reserves room
// for it through --vp-layout-top-height, set in custom.css alongside the styles.
export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      'layout-top': () =>
        h('div', { class: 'archived-banner', role: 'note' }, [
          h('strong', null, 'This project is archived.'),
          ' It is no longer maintained. The code stays available and works as documented.',
        ]),
    })
  },
}
