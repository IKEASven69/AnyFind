#!/usr/bin/env node
// build.mjs — dsh-anyfind 构建：
//   ① host 半：esbuild 打成 ESM 自包含 bundle（lib/index.js）。运行时依赖只有
//      @deepseek-ai/dsh-tools 的 defineTool（纯 JS，内联打进产物）与 node: 内置
//      （外置）——对齐 dsh-hippo tsdown alwaysBundle 的"目录外 link 装配不装 peers"
//      教训：产物不能留 @deepseek-ai/* 的运行时裸引用。
//   ② client 半：esbuild 打成 CJS，包进 window.__ModuleLoader__.load({id, factory})
//      自注册壳（dsh-hippo scripts/build-client.mjs 同型）；react 由宿主运行时注入。
import { build } from 'esbuild'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const LIB = join(ROOT, 'lib')
mkdirSync(LIB, { recursive: true })

// ① host 半
await build({
  entryPoints: [join(ROOT, 'src', 'index.ts')],
  bundle: true,
  format: 'esm',
  platform: 'node',
  target: 'es2022',
  external: ['node:*'],
  minify: false,
  sourcemap: true,
  outfile: join(LIB, 'index.js'),
  logLevel: 'info',
})

// ② client 半（CJS + __ModuleLoader__ 壳）
const result = await build({
  entryPoints: [join(ROOT, 'src', 'client.ts')],
  bundle: true,
  format: 'cjs',
  platform: 'browser',
  target: 'es2022',
  external: ['react', 'react/jsx-runtime'],
  minify: false,
  write: false,
  sourcemap: false,
  logLevel: 'info',
})

const code = result.outputFiles[0].text
// react 自由变量注入：src/vendor/**（dsh-workspace-explorer vendor，见 src/vendor/VENDORED.md）
// 模块顶层用 `react.createElement`，需在工厂作用域提供 react（上游 build-client.mjs 同型）。
const wrapped = `window.__ModuleLoader__.load({
	id: "dsh-anyfind",
	factory: (require) => {
		var react = require("react");
		var module = { exports: {} };
		var exports = module.exports;
${code.replace(/^/gm, '\t\t')}
		return module.exports;
	}
});
`
writeFileSync(join(LIB, 'client.js'), wrapped, 'utf8')
console.log(`  wrote ${join(LIB, 'client.js')} (${wrapped.length} bytes)`)
