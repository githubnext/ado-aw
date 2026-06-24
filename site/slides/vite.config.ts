import { defineConfig } from 'vite'

// Deploy the deck under a sub-path (…/ado-aw/slides/) without breaking Slidev's
// in-app navigation. Slidev's getSlidePath() prepends import.meta.env.BASE_URL
// to every router target, so a non-"/" Vite base leaks into (or doubles in) the
// route path — which breaks presenter mode in particular.
//
// Fix: keep the Vite base at "/" (so BASE_URL stays "/", and hash routes resolve
// cleanly as /N and /presenter/N), but emit *relative* asset URLs via
// renderBuiltUrl so the bundle still loads correctly from the sub-path.
export default defineConfig({
  experimental: {
    renderBuiltUrl() {
      return { relative: true }
    },
  },
})
