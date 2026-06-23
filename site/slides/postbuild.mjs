// Post-build step for the sub-path deployment of the Slidev deck.
//
// The deck is built with the default Vite base "/" so that Slidev's in-app
// navigation (getSlidePath → import.meta.env.BASE_URL) produces clean hash
// routes (/N and /presenter/N) for both play and presenter mode. JS/CSS asset
// URLs are emitted relative via experimental.renderBuiltUrl (see vite.config.ts).
//
// The only thing left pointing at an absolute "/assets/…" path is the entry
// markup in the generated HTML files. Rewrite those to "./assets/…" so the
// bundle also loads correctly when served from …/ado-aw/slides/.
import { readdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'

const outDir = process.argv[2]
if (!outDir) {
  console.error('usage: node postbuild.mjs <out-dir>')
  process.exit(1)
}

const entries = await readdir(outDir, { withFileTypes: true })
let patched = 0
for (const e of entries) {
  if (!e.isFile() || !e.name.endsWith('.html')) continue
  const fp = path.join(outDir, e.name)
  const src = await readFile(fp, 'utf8')
  const out = src.replaceAll('="/assets/', '="./assets/')
  if (out !== src) {
    await writeFile(fp, out)
    patched++
    console.log(`  rewrote ${e.name}`)
  }
}
console.log(`postbuild: relative-ized assets in ${patched} HTML file(s)`)
