# Changelog

## [0.35.3](https://github.com/githubnext/ado-aw/compare/v0.35.2...v0.35.3) (2026-06-11)


### Bug Fixes

* **ado-script:** treat unsubstituted ADO macros as empty in synthPr ([#975](https://github.com/githubnext/ado-aw/issues/975)) ([1164c5f](https://github.com/githubnext/ado-aw/commit/1164c5fefbd2d06d46d46e8c64d869f29a6452a2))

## [0.35.2](https://github.com/githubnext/ado-aw/compare/v0.35.1...v0.35.2) (2026-06-11)


### Bug Fixes

* **compile:** unify synthetic-PR variable namespace via ado-script ([#972](https://github.com/githubnext/ado-aw/issues/972)) ([0e05b3a](https://github.com/githubnext/ado-aw/commit/0e05b3a01975d23363d65122cd724174f5c1d329))

## [0.35.1](https://github.com/githubnext/ado-aw/compare/v0.35.0...v0.35.1) (2026-06-11)


### Bug Fixes

* **compile:** repair synthetic-PR gate + Stage step env var plumbing ([#956](https://github.com/githubnext/ado-aw/issues/956)) ([79919bd](https://github.com/githubnext/ado-aw/commit/79919bd2280f0fb17a118df17f824a0e9e504691))
* **workflows:** allow github-actions[bot] to trigger recompile-safe-output-fixtures ([#958](https://github.com/githubnext/ado-aw/issues/958)) ([5d36dfd](https://github.com/githubnext/ado-aw/commit/5d36dfdc4e214b7b056fa5f9c94586941ced7d8a))

## [0.35.0](https://github.com/githubnext/ado-aw/compare/v0.34.3...v0.35.0) (2026-06-10)


### Features

* **install:** add checksum-verified one-line first-time installers ([#942](https://github.com/githubnext/ado-aw/issues/942)) ([040cdad](https://github.com/githubnext/ado-aw/commit/040cdad14f90dd44767dcb84e15a0c9d48fe8a55))

## [0.34.3](https://github.com/githubnext/ado-aw/compare/v0.34.2...v0.34.3) (2026-06-10)


### Bug Fixes

* **compile:** use macro form for same-job synthPr gate refs ([#944](https://github.com/githubnext/ado-aw/issues/944)) ([51ae40e](https://github.com/githubnext/ado-aw/commit/51ae40ee4a63f1ae99faee9d47bda158811b5069))
* **workflows:** lower min-integrity for issue-plan-maker /plan command ([#952](https://github.com/githubnext/ado-aw/issues/952)) ([2808e8a](https://github.com/githubnext/ado-aw/commit/2808e8acd14cf729b2dbd6aeb6b37e287ec3db80))

## [0.34.2](https://github.com/githubnext/ado-aw/compare/v0.34.1...v0.34.2) (2026-06-09)


### Bug Fixes

* **compile:** gate exec-context-pr synth path in bash, not step condition ([#937](https://github.com/githubnext/ado-aw/issues/937)) ([6e67699](https://github.com/githubnext/ado-aw/commit/6e6769974ee56b7b840d8f104a789093cdf00743))

## [0.34.1](https://github.com/githubnext/ado-aw/compare/v0.34.0...v0.34.1) (2026-06-09)


### Bug Fixes

* **ado-script:** inline azure-devops-node-api to avoid missing ncc chunk ([#935](https://github.com/githubnext/ado-aw/issues/935)) ([7a568fa](https://github.com/githubnext/ado-aw/commit/7a568fae8e42710d7905979060a2c20de3ad7982))

## [0.34.0](https://github.com/githubnext/ado-aw/compare/v0.33.1...v0.34.0) (2026-06-09)


### ⚠ BREAKING CHANGES

* **compile:** record on.pr.mode (synthetic|policy) trigger model ([#933](https://github.com/githubnext/ado-aw/issues/933))

### Features

* **compile:** record on.pr.mode (synthetic|policy) trigger model ([#933](https://github.com/githubnext/ado-aw/issues/933)) ([925e3ee](https://github.com/githubnext/ado-aw/commit/925e3eeb8c9f41693a1408981dd0435398888aef))


### Bug Fixes

* **release:** use built-in GITHUB_TOKEN to dispatch recompile workflow ([#923](https://github.com/githubnext/ado-aw/issues/923)) ([ce617a7](https://github.com/githubnext/ado-aw/commit/ce617a7b019f85b7860733fd33dc4fe59bb78831))

## [0.33.1](https://github.com/githubnext/ado-aw/compare/v0.33.0...v0.33.1) (2026-06-09)


### Bug Fixes

* **gate:** use '.' separator in build tags so ADO doesn't reject ':' in REST path ([#917](https://github.com/githubnext/ado-aw/issues/917)) ([4122a64](https://github.com/githubnext/ado-aw/commit/4122a64086ba9e64b6fbda8afcee3398f4d10dad))


### Performance Improvements

* **secrets:** prune disabled/paused pipelines by default in discovery, add --include-disabled opt-out ([#914](https://github.com/githubnext/ado-aw/issues/914)) ([f7e9d17](https://github.com/githubnext/ado-aw/commit/f7e9d17e3efcf68e2c6670ee1adf5e12d3f760c6))

## [0.33.0](https://github.com/githubnext/ado-aw/compare/v0.32.0...v0.33.0) (2026-06-08)


### Features

* **enable:** support GitHub-source pipelines via --service-connection ([#905](https://github.com/githubnext/ado-aw/issues/905)) ([6ab5f9a](https://github.com/githubnext/ado-aw/commit/6ab5f9a50efb44858e166813fe6ea318548353d6))


### Bug Fixes

* **test:** permit Stage 3 executor SYSTEM_ACCESSTOKEN env mapping ([#887](https://github.com/githubnext/ado-aw/issues/887)) ([e3beb0f](https://github.com/githubnext/ado-aw/commit/e3beb0ff6e15d377d061bcf4fc1ee95e72336215))

## [0.32.0](https://github.com/githubnext/ado-aw/compare/v0.31.1...v0.32.0) (2026-06-07)


### Features

* **audit:** add `ado-aw audit <build-id-or-url>` command ([#691](https://github.com/githubnext/ado-aw/issues/691)) ([5c33e40](https://github.com/githubnext/ado-aw/commit/5c33e40d1d51a32077597046f490d1ab52ed8111))
* **compile:** add execution-context plugin with PR contributor ([#860](https://github.com/githubnext/ado-aw/issues/860)) ([#865](https://github.com/githubnext/ado-aw/issues/865)) ([3de0690](https://github.com/githubnext/ado-aw/commit/3de0690ac7b09ba7ec4c340f45be685a047c0168))
* **compile:** default executor to System.AccessToken and add always-on Azure CLI ([#873](https://github.com/githubnext/ado-aw/issues/873)) ([ca4a04b](https://github.com/githubnext/ado-aw/commit/ca4a04b156e0631c211eef711bbf6e81d7f9ebc8))
* **workflows:** recompile tests/safe-outputs fixtures on each ado-aw release ([#863](https://github.com/githubnext/ado-aw/issues/863)) ([7c6fb23](https://github.com/githubnext/ado-aw/commit/7c6fb230184bad74542ead0a74de1d9a3da5a105))


### Bug Fixes

* **workflows:** add integrity heuristic and per-file compile loop to recompile-safe-output-fixtures ([#868](https://github.com/githubnext/ado-aw/issues/868)) ([665ad0e](https://github.com/githubnext/ado-aw/commit/665ad0e461f09efc2e2ec3e37ff9595507da62a0))
* **workflows:** allow github CDN egress in recompile-safe-output-fixtures ([#864](https://github.com/githubnext/ado-aw/issues/864)) ([abc4959](https://github.com/githubnext/ado-aw/commit/abc49599f97bfe7230e19cfbd9e7e763d55085db))
* **workflows:** broaden allowed-files glob to cover flat tests/safe-outputs layout ([#871](https://github.com/githubnext/ado-aw/issues/871)) ([6d3559f](https://github.com/githubnext/ado-aw/commit/6d3559f7f789a4543ed7edeb6e67fb41aeb8ef4f))
* **workflows:** switch recompile trigger to release:published and drop broken Step 0 ([#869](https://github.com/githubnext/ado-aw/issues/869)) ([039e547](https://github.com/githubnext/ado-aw/commit/039e5478b978cca0853e87313d5d9f2a63b76822))

## [0.31.1](https://github.com/githubnext/ado-aw/compare/v0.31.0...v0.31.1) (2026-06-01)


### Bug Fixes

* **compile:** allow external dependsOn/condition on job and stage template targets ([#823](https://github.com/githubnext/ado-aw/issues/823)) ([608b0b5](https://github.com/githubnext/ado-aw/commit/608b0b56331ce2be1f8461fc13c80b3958426ea5))
* **workflows:** allow doc-freshness-check to patch safeoutput source files ([#816](https://github.com/githubnext/ado-aw/issues/816)) ([d16f808](https://github.com/githubnext/ado-aw/commit/d16f808c1d68c7c05b39af973af9b5129144eb13))

## [0.31.0](https://github.com/githubnext/ado-aw/compare/v0.30.2...v0.31.0) (2026-06-01)


### Features

* **cli:** check for newer GitHub release on every user-facing command ([#637](https://github.com/githubnext/ado-aw/issues/637)) ([7d197f1](https://github.com/githubnext/ado-aw/commit/7d197f1553ad037af39cb78cab679a437ceb9fd1))
* **compile:** runtime prompt loading via {{#runtime-import}} markers ([#625](https://github.com/githubnext/ado-aw/issues/625)) ([6ad1816](https://github.com/githubnext/ado-aw/commit/6ad18164e47c76e54502e0347aa5f59ce93c8bf2))
* **compile:** warn about out-of-date compiled definitions on recompile ([#638](https://github.com/githubnext/ado-aw/issues/638)) ([1b2b266](https://github.com/githubnext/ado-aw/commit/1b2b2664a3b988540362a290f3d84ecdab440939))
* **secrets:** add --all-repos and --source via Pipeline Preview discovery ([#624](https://github.com/githubnext/ado-aw/issues/624)) ([6d4c824](https://github.com/githubnext/ado-aw/commit/6d4c8247242de8ed50cfa7903ec76d38744f5859))
* **workflows:** add docs-writer agentic workflow ([#631](https://github.com/githubnext/ado-aw/issues/631)) ([50bd4e0](https://github.com/githubnext/ado-aw/commit/50bd4e0ffd72097b74141762f44dcf45eae003d3))
* **workflows:** add test-reducer agentic workflow ([#626](https://github.com/githubnext/ado-aw/issues/626)) ([4733c76](https://github.com/githubnext/ado-aw/commit/4733c7604c705bcece84e3e9f6f4770aaa591e5c))
* **workflows:** make test-gap-finder open PRs for coverage gaps ([#680](https://github.com/githubnext/ado-aw/issues/680)) ([4087579](https://github.com/githubnext/ado-aw/commit/4087579fa62002e773b957dd6c9fb3f906d90d61))


### Bug Fixes

* **compile:** normalize absolute input paths in generate_header_comment ([#645](https://github.com/githubnext/ado-aw/issues/645)) ([7a0d268](https://github.com/githubnext/ado-aw/commit/7a0d2687e9280c8816c5db0dcb2ee024b052418c))
* **site:** create runtime-imports page and fix broken relative link in filter-ir.mdx ([#654](https://github.com/githubnext/ado-aw/issues/654)) ([ad1b44b](https://github.com/githubnext/ado-aw/commit/ad1b44be257a3630ad09b4d29b8f8b3d6ade048e))
* **site:** restore correct --sl-color-black in light mode for search box contrast ([#646](https://github.com/githubnext/ado-aw/issues/646)) ([f63a1a6](https://github.com/githubnext/ado-aw/commit/f63a1a663eccfe731c8210ed5f9d9d38c17233f8))

## [0.30.2](https://github.com/githubnext/ado-aw/compare/v0.30.1...v0.30.2) (2026-05-18)


### Bug Fixes

* **compile:** resolve autodiscovered source path relative to lock file directory ([#616](https://github.com/githubnext/ado-aw/issues/616)) ([be2e1c4](https://github.com/githubnext/ado-aw/commit/be2e1c4f55930846398650b55d430ec4e328c8d4))

## [0.30.1](https://github.com/githubnext/ado-aw/compare/v0.30.0...v0.30.1) (2026-05-18)


### Bug Fixes

* **tests:** isolate fixtures from codemod rewrites and bump Windows debug stack ([#613](https://github.com/githubnext/ado-aw/issues/613)) ([a628d09](https://github.com/githubnext/ado-aw/commit/a628d09c4b00a2a61a02c3b213c7a2b9e944f5d0))

## [0.30.0](https://github.com/githubnext/ado-aw/compare/v0.29.0...v0.30.0) (2026-05-18)


### Features

* **cli:** add ado-aw enable ([#583](https://github.com/githubnext/ado-aw/issues/583)) ([1b4273b](https://github.com/githubnext/ado-aw/commit/1b4273b860ba58db7381bb0f61b36158682df4c7))
* **cli:** consolidate Phase 1 pipeline-lifecycle commands (disable/remove/list/status/run/secrets) ([#602](https://github.com/githubnext/ado-aw/issues/602)) ([096b7a4](https://github.com/githubnext/ado-aw/commit/096b7a48ff35a8205c55aaa8954eec6ffc7f9623))
* **compile:** add --force flag to bypass GitHub-remote guard ([#577](https://github.com/githubnext/ado-aw/issues/577)) ([562ff7f](https://github.com/githubnext/ado-aw/commit/562ff7fd92b56e810a2ea247d08c1a212e044136))
* **compile:** replace Python gate evaluator with bundled TypeScript gate.js ([#389](https://github.com/githubnext/ado-aw/issues/389)) ([a2ce38c](https://github.com/githubnext/ado-aw/commit/a2ce38c7dbf027e8c9cefb1c3bb37b850cb5111f))
* **engine:** split copilot CLI install path by compile target ([#584](https://github.com/githubnext/ado-aw/issues/584)) ([4fe2175](https://github.com/githubnext/ado-aw/commit/4fe2175861bbfe5ab6c02ba06cf7e68e010e047a))
* **safeoutputs:** add daily smoke suite and reject unknown safe-output keys ([#563](https://github.com/githubnext/ado-aw/issues/563)) ([29b221e](https://github.com/githubnext/ado-aw/commit/29b221e8a613ee11cac667368a6063e5c8149218))
* **workflows:** add clippy-fixer agentic workflow ([#575](https://github.com/githubnext/ado-aw/issues/575)) ([ca914f1](https://github.com/githubnext/ado-aw/commit/ca914f1dd814f0d1a2e70b5e2005e96051d928d1))
* **workflows:** allow update-awf-version to close superseded PRs ([#590](https://github.com/githubnext/ado-aw/issues/590)) ([e0ddc2c](https://github.com/githubnext/ado-aw/commit/e0ddc2ca24a324cbaf73756313cc01e7ee2e1d78))
* **workflows:** file release-notes action-item issues from update-awf-version ([#593](https://github.com/githubnext/ado-aw/issues/593)) ([d4fedd5](https://github.com/githubnext/ado-aw/commit/d4fedd511bd0d88170f2033f80a925bc0a972819))


### Bug Fixes

* **compile:** default non-1es vmImage to ubuntu-22.04 ([#578](https://github.com/githubnext/ado-aw/issues/578)) ([b841998](https://github.com/githubnext/ado-aw/commit/b8419985215c56eb6280a9bf3e6c50534512fc83))
* **compile:** enforce ADO build-number rules for pipeline_agent_name ([#576](https://github.com/githubnext/ado-aw/issues/576)) ([1c7c407](https://github.com/githubnext/ado-aw/commit/1c7c407dfc339dea14becea6e22861669ef85255))
* **compile:** quote pipeline name in generated YAML to handle colons and quotes ([#568](https://github.com/githubnext/ado-aw/issues/568)) ([ebd2f17](https://github.com/githubnext/ado-aw/commit/ebd2f170aceab8ece5e04c946ad8adf89e59ec05))
* **compile:** tighten ADO org name validation to alphanumeric and hyphen only ([#598](https://github.com/githubnext/ado-aw/issues/598)) ([91d39a9](https://github.com/githubnext/ado-aw/commit/91d39a9acb6cabb53c7ee6b9f7df7c942abdbd93))
* **configure:** accept bare org name for --org ([#579](https://github.com/githubnext/ado-aw/issues/579)) ([8029980](https://github.com/githubnext/ado-aw/commit/8029980aceb432ec52c3b074fd1ad946b8cb3aa1))
* **safeoutputs:** prevent symlink exfiltration in create-pr Stage 3 ([#549](https://github.com/githubnext/ado-aw/issues/549)) ([f04c033](https://github.com/githubnext/ado-aw/commit/f04c0338525466bb7ffad050c203a6c9639b5e7a))
* **secrets:** preserve masked ADO secrets on definition PUT ([#604](https://github.com/githubnext/ado-aw/issues/604)) ([2e0a0bb](https://github.com/githubnext/ado-aw/commit/2e0a0bb5b6d4396701d5967c2a0f2c82be0a39c8))

## [0.29.0](https://github.com/githubnext/ado-aw/compare/v0.28.0...v0.29.0) (2026-05-15)


### Features

* **compile:** add target: job and target: stage for ADO template output ([#519](https://github.com/githubnext/ado-aw/issues/519)) ([8df9682](https://github.com/githubnext/ado-aw/commit/8df9682f664bf00011a1a32b9a4bc62b09268a56))
* **compile:** unify pool front-matter replacement across targets ([#538](https://github.com/githubnext/ado-aw/issues/538)) ([7806369](https://github.com/githubnext/ado-aw/commit/7806369766d3837de5a11cdf9f359ec2a63c59b3))
* **safeoutputs:** add ado-aw-debug.create-issue for dogfood pipelines ([#492](https://github.com/githubnext/ado-aw/issues/492)) ([69a634b](https://github.com/githubnext/ado-aw/commit/69a634b436ae7fa62bd5a1eaffd490a27d7b82fb))
* **safeoutputs:** add work-item filing to noop and missing-tool safe outputs ([#521](https://github.com/githubnext/ado-aw/issues/521)) ([c1bf552](https://github.com/githubnext/ado-aw/commit/c1bf552440be7f39108eef6266d121970ade1fc7))


### Bug Fixes

* **cache-memory:** reject symlinks in agent memory to prevent Stage 3 credential theft ([#524](https://github.com/githubnext/ado-aw/issues/524)) ([f311d36](https://github.com/githubnext/ado-aw/commit/f311d364258ba0b3faae7ac5629f3e56b5c3950a))
* **ci:** move docs deploy workflow to top-level .github/workflows ([#539](https://github.com/githubnext/ado-aw/issues/539)) ([dcb2b33](https://github.com/githubnext/ado-aw/commit/dcb2b33fa1d06280a08bd3caf0589d08960350d7))
* **compile:** address pool review feedback ([#541](https://github.com/githubnext/ado-aw/issues/541)) ([9bdb126](https://github.com/githubnext/ado-aw/commit/9bdb12606c5567ab5b31b5234be0f795c9e90817))
* **safeoutputs:** neutralize Stage 3 upload message command injection paths ([#501](https://github.com/githubnext/ado-aw/issues/501)) ([45cd552](https://github.com/githubnext/ado-aw/commit/45cd55271d0a8fc8d273f7ce11ac501030f7b9d6))

## [0.28.0](https://github.com/githubnext/ado-aw/compare/v0.27.0...v0.28.0) (2026-05-09)


### Features

* **compile:** add compact `repos:` front-matter syntax with codemod auto-rewrite ([#478](https://github.com/githubnext/ado-aw/issues/478)) ([3e67cfd](https://github.com/githubnext/ado-aw/commit/3e67cfd30ea28153890460bf36ba4db26bce9e30))
* **compile:** autorewrite front matter via detection-based codemods ([#476](https://github.com/githubnext/ado-aw/issues/476)) ([64bbc73](https://github.com/githubnext/ado-aw/commit/64bbc73a4ea3d53d4bed14ddc4c0166ac6d36887))


### Bug Fixes

* **safeoutputs:** block VSO command injection via repository alias across Stage 3 PR-safe-output executors ([#482](https://github.com/githubnext/ado-aw/issues/482)) ([6f4a6dd](https://github.com/githubnext/ado-aw/commit/6f4a6dd2642521067bcf955c3db0912245061e8a))
* **workflow:** add protected-files fallback-to-issue for doc-freshness-check ([#473](https://github.com/githubnext/ado-aw/issues/473)) ([12e273c](https://github.com/githubnext/ado-aw/commit/12e273c18fd3526d0c3d1020a215052b94f196d1))

## [0.27.0](https://github.com/githubnext/ado-aw/compare/v0.26.1...v0.27.0) (2026-05-08)


### Features

* **cli:** make `init` always overwrite; `--force` bypasses GitHub remote guard ([#465](https://github.com/githubnext/ado-aw/issues/465)) ([6c1a332](https://github.com/githubnext/ado-aw/commit/6c1a332d6f21bff797a6ca963c5ed4050d47dde4))


### Bug Fixes

* **doc-freshness-check:** expand allowed-files to docs, README, prompts ([#463](https://github.com/githubnext/ado-aw/issues/463)) ([4abeb09](https://github.com/githubnext/ado-aw/commit/4abeb09899674f1f68143eb5f509c71fccdcf9db))
* **execute:** match safe-output repository by name or alias ([#469](https://github.com/githubnext/ado-aw/issues/469)) ([76cd618](https://github.com/githubnext/ado-aw/commit/76cd6180e3bf5aafc84ffc95f7b23ed444d61dac))
* **safeoutputs:** send Content-Range header for pipeline artifact uploads ([#467](https://github.com/githubnext/ado-aw/issues/467)) ([e95cbf5](https://github.com/githubnext/ado-aw/commit/e95cbf5a87419ee37c16e1b53c7d81d8fe607523))

## [0.26.1](https://github.com/githubnext/ado-aw/compare/v0.26.0...v0.26.1) (2026-05-08)


### Bug Fixes

* **ci:** allow doc freshness workflow to update AGENTS.md ([#452](https://github.com/githubnext/ado-aw/issues/452)) ([463dafa](https://github.com/githubnext/ado-aw/commit/463dafa18fa0c02bbf25c0d103e64e97257a0096))
* **compile:** detect and prevent workspace checkout collision with self repo ([#456](https://github.com/githubnext/ado-aw/issues/456)) ([051d45c](https://github.com/githubnext/ado-aw/commit/051d45cc90b900028612b4210bafbd5e7e65372e))

## [0.26.0](https://github.com/githubnext/ado-aw/compare/v0.25.1...v0.26.0) (2026-05-07)


### Features

* **workflows:** add frontmatter-aligner gh-aw workflow ([#448](https://github.com/githubnext/ado-aw/issues/448)) ([d3af7e6](https://github.com/githubnext/ado-aw/commit/d3af7e667034a42fb534becb820b0dadb7a9118a))


### Bug Fixes

* **cli:** direct GitHub repos to gh-aw for compile/init ([#447](https://github.com/githubnext/ado-aw/issues/447)) ([0ca3a25](https://github.com/githubnext/ado-aw/commit/0ca3a2530cfbb64d3d8c7c8cdc4d4213b7b05008))

## [0.25.1](https://github.com/githubnext/ado-aw/compare/v0.25.0...v0.25.1) (2026-05-07)


### Bug Fixes

* **safeoutputs:** support glob wildcards anywhere in allowed-tags patterns ([#442](https://github.com/githubnext/ado-aw/issues/442)) ([17e372c](https://github.com/githubnext/ado-aw/commit/17e372ca42a992aed5a24ef30e89d16977d7bb39))
* **workflows:** simplify change-risk to use add-comment instead of PR review ([#443](https://github.com/githubnext/ado-aw/issues/443)) ([4f2f188](https://github.com/githubnext/ado-aw/commit/4f2f188f38132cf5151720ffd33e06db12e1c9f4))

## [0.25.0](https://github.com/githubnext/ado-aw/compare/v0.24.0...v0.25.0) (2026-05-07)


### Features

* **runtimes:** add dotnet runtime extension ([#435](https://github.com/githubnext/ado-aw/issues/435)) ([bdfb21c](https://github.com/githubnext/ado-aw/commit/bdfb21cfeb08544ceea7829e9614d07516cf35e1))


### Bug Fixes

* **compile:** fail pipeline step on AWF download errors ([#439](https://github.com/githubnext/ado-aw/issues/439)) ([367dd9d](https://github.com/githubnext/ado-aw/commit/367dd9d0000e4de2cd5b1f4c63c499907e6b70ff))

## [0.24.0](https://github.com/githubnext/ado-aw/compare/v0.23.1...v0.24.0) (2026-05-07)


### Features

* **workflows:** add /change-risk slash command for PR risk assessment ([#434](https://github.com/githubnext/ado-aw/issues/434)) ([e787956](https://github.com/githubnext/ado-aw/commit/e787956a9f9519368905affaa71ab32e47d907d6))


### Bug Fixes

* **logging:** always capture debug logs to file while preserving console verbosity ([#430](https://github.com/githubnext/ado-aw/issues/430)) ([64e9709](https://github.com/githubnext/ado-aw/commit/64e97091c2dd36d08de0315c2c71cc676e9169a1))
* **safeoutputs:** use sanitize_config for identifier fields instead of sanitize_text ([#433](https://github.com/githubnext/ado-aw/issues/433)) ([ea43b11](https://github.com/githubnext/ado-aw/commit/ea43b11c8b6b63854233289f42667bf621aef3a6))

## [0.23.1](https://github.com/githubnext/ado-aw/compare/v0.23.0...v0.23.1) (2026-05-07)


### Bug Fixes

* **safeoutputs:** rewrite upload-pipeline-artifact flow to fix HTTP 405 ([#425](https://github.com/githubnext/ado-aw/issues/425)) ([5e4c89e](https://github.com/githubnext/ado-aw/commit/5e4c89eeeb7a401bc677f2a8e3a65832a6aebdf4))
* **security:** harden upload path validation and trigger filter script integrity ([#428](https://github.com/githubnext/ado-aw/issues/428)) ([84a2031](https://github.com/githubnext/ado-aw/commit/84a203100e935585b805792cb9453dc30955a6c2))

## [0.23.0](https://github.com/githubnext/ado-aw/compare/v0.22.4...v0.23.0) (2026-05-06)


### Features

* **safeoutputs:** add dynamic tags with allowed-tags to create/update work item ([#420](https://github.com/githubnext/ado-aw/issues/420)) ([d02997a](https://github.com/githubnext/ado-aw/commit/d02997a1a704df6ac6e0233adb8a9aa5424927cb))


### Bug Fixes

* **safeoutputs:** fix upload-pipeline-artifact 405 by adding scopeIdentifier to container API requests ([#421](https://github.com/githubnext/ado-aw/issues/421)) ([2d460fd](https://github.com/githubnext/ado-aw/commit/2d460fdce89ce08927d659f76617668146284bc9))

## [0.22.4](https://github.com/githubnext/ado-aw/compare/v0.22.3...v0.22.4) (2026-05-06)


### Bug Fixes

* **execute:** don't overwrite env-derived ADO context with None CLI args ([#413](https://github.com/githubnext/ado-aw/issues/413)) ([aa8a09f](https://github.com/githubnext/ado-aw/commit/aa8a09f294629c78bb10686333b499a2f828a8e0))

## [0.22.3](https://github.com/githubnext/ado-aw/compare/v0.22.2...v0.22.3) (2026-05-05)


### Bug Fixes

* **compile:** show full error chain in batch compile output ([#411](https://github.com/githubnext/ado-aw/issues/411)) ([157ed02](https://github.com/githubnext/ado-aw/commit/157ed02bde1253cc4e31715187962161cd73b256))

## [0.22.2](https://github.com/githubnext/ado-aw/compare/v0.22.1...v0.22.2) (2026-05-05)


### Bug Fixes

* **compile:** reject unknown front-matter fields with deny_unknown_fields ([#409](https://github.com/githubnext/ado-aw/issues/409)) ([788250d](https://github.com/githubnext/ado-aw/commit/788250d9652a8e227688b3d75b51c2c423017395))

## [0.22.1](https://github.com/githubnext/ado-aw/compare/v0.22.0...v0.22.1) (2026-05-05)


### Bug Fixes

* **compile:** remove empty env block from executor step when no write permissions ([#407](https://github.com/githubnext/ado-aw/issues/407)) ([0f25f06](https://github.com/githubnext/ado-aw/commit/0f25f060747c669f2c8b2c9f044c46a5b71ebcc2))

## [0.22.0](https://github.com/githubnext/ado-aw/compare/v0.21.0...v0.22.0) (2026-05-05)


### Features

* **runtimes:** add unified Node.js and Python runtime extensions ([#400](https://github.com/githubnext/ado-aw/issues/400)) ([991621a](https://github.com/githubnext/ado-aw/commit/991621a2843f29e7590a5d033b6a0eecceebdab5))
* **safe-outputs:** rename upload-build-artifact to upload-build-attachment and add upload-pipeline-artifact ([#404](https://github.com/githubnext/ado-aw/issues/404)) ([d3aad31](https://github.com/githubnext/ado-aw/commit/d3aad31ced47309c7c9388e816f0981e1e769d8d))


### Bug Fixes

* **security:** neutralize pipeline commands in execute_safe_outputs Err arm ([#405](https://github.com/githubnext/ado-aw/issues/405)) ([da83c03](https://github.com/githubnext/ado-aw/commit/da83c0312070e7fab2ef3329734c3680b9239210))
* **security:** neutralize VSO pipeline commands in Stage 3 log output ([#396](https://github.com/githubnext/ado-aw/issues/396)) ([ea76888](https://github.com/githubnext/ado-aw/commit/ea7688812b951c923abaee0514e5e0c46d1826ed))

## [0.21.0](https://github.com/githubnext/ado-aw/compare/v0.20.0...v0.21.0) (2026-05-02)


### ⚠ BREAKING CHANGES

* **compile:** trigger filter IR with data-driven Python evaluator ([#345](https://github.com/githubnext/ado-aw/issues/345))

### Features

* **compile:** trigger filter IR with data-driven Python evaluator ([#345](https://github.com/githubnext/ado-aw/issues/345)) ([90df351](https://github.com/githubnext/ado-aw/commit/90df351d3ff5a752265c5813f7f5f3ead8256411))
* **executor:** auto-capture ADO build variables in ExecutionContext ([#378](https://github.com/githubnext/ado-aw/issues/378)) ([7575218](https://github.com/githubnext/ado-aw/commit/7575218d288e081082804a82c9af2ca499fbaff5))
* **safe-outputs:** add unified upload-build-artifact safe output ([#380](https://github.com/githubnext/ado-aw/issues/380)) ([b66037d](https://github.com/githubnext/ado-aw/commit/b66037d4b093baa18cb04dfa0792f9fca6d2f9e1))


### Bug Fixes

* **safeoutputs:** enforce add-build-tag scope for build IDs &gt; i32::MAX ([#379](https://github.com/githubnext/ado-aw/issues/379)) ([c533900](https://github.com/githubnext/ado-aw/commit/c53390002c8d7a9d9fa2e83fdb3e636da8f51250))
* **safeoutputs:** sanitize ADO-sourced title and tags in prefix-guard error messages to prevent VSO command injection ([#370](https://github.com/githubnext/ado-aw/issues/370)) ([3fa067f](https://github.com/githubnext/ado-aw/commit/3fa067fd6bee4e14142f879a8011cf992f6b7c92))

## [0.20.0](https://github.com/githubnext/ado-aw/compare/v0.19.0...v0.20.0) (2026-04-29)


### Features

* **compile:** add awf_path_prepends for chroot PATH injection ([#359](https://github.com/githubnext/ado-aw/issues/359)) ([4576bc3](https://github.com/githubnext/ado-aw/commit/4576bc3978543f52f171acf3a1792b6042d4ff33))

## [0.19.0](https://github.com/githubnext/ado-aw/compare/v0.18.2...v0.19.0) (2026-04-29)


### Features

* **compile:** add required_awf_mounts to CompilerExtension for Lean runtime ([#354](https://github.com/githubnext/ado-aw/issues/354)) ([f6f437b](https://github.com/githubnext/ado-aw/commit/f6f437bfd38bd2b6ef5665d55a830f2312a48dc2))
* **engine:** update default model to claude-opus-4.7 ([#355](https://github.com/githubnext/ado-aw/issues/355)) ([dff681b](https://github.com/githubnext/ado-aw/commit/dff681b21ccc6df07d4f9ef7a1642517c933e5dd))

## [0.18.2](https://github.com/githubnext/ado-aw/compare/v0.18.1...v0.18.2) (2026-04-28)


### Bug Fixes

* **compile:** use workingDirectory for integrity check instead of absolute path ([#346](https://github.com/githubnext/ado-aw/issues/346)) ([0ce9238](https://github.com/githubnext/ado-aw/commit/0ce9238331d2983cecfc20e9a6a1de96d3bf0938))

## [0.18.1](https://github.com/githubnext/ado-aw/compare/v0.18.0...v0.18.1) (2026-04-28)


### Bug Fixes

* **compile:** anchor source/pipeline paths to trigger repo, not workspace ([#342](https://github.com/githubnext/ado-aw/issues/342)) ([0845490](https://github.com/githubnext/ado-aw/commit/0845490ab874a84d98f6edc33f8482371ca52d73))
* **compile:** pin LF eol on managed gitattributes entries ([#340](https://github.com/githubnext/ado-aw/issues/340)) ([8efb5fb](https://github.com/githubnext/ado-aw/commit/8efb5fb1bb1b4e73b5b84c5e16ed862ca39c615a))

## [0.18.0](https://github.com/githubnext/ado-aw/compare/v0.17.1...v0.18.0) (2026-04-28)


### Features

* trigger release for unreleased changes ([#338](https://github.com/githubnext/ado-aw/issues/338)) ([b8c899b](https://github.com/githubnext/ado-aw/commit/b8c899b5d61eedc7fdd413b0b77f3e0d366d0e23))

## [0.17.1](https://github.com/githubnext/ado-aw/compare/v0.17.0...v0.17.1) (2026-04-27)


### Bug Fixes

* block template marker delimiters in front matter identity fields ([#315](https://github.com/githubnext/ado-aw/issues/315)) ([2575246](https://github.com/githubnext/ado-aw/commit/2575246ef14b958e4baf757688190d6109286bf4))
* deterministic MCPG config key ordering (HashMap -&gt; BTreeMap) ([#328](https://github.com/githubnext/ado-aw/issues/328)) ([889ee62](https://github.com/githubnext/ado-aw/commit/889ee62143d535ec029ef2a71e6fe9d0de23d297))

## [0.17.0](https://github.com/githubnext/ado-aw/compare/v0.16.0...v0.17.0) (2026-04-23)


### ⚠ BREAKING CHANGES

* The `run` subcommand is removed. See docs/local-development.md for manual local development instructions.
* The engine front matter format changed from model names (engine: claude-opus-4.5) to engine identifiers (engine: copilot). Model is now a sub-field in the object form (engine: { id: copilot, model: ... }).

### Features

* add cyclomatic complexity reducer agentic workflow ([#298](https://github.com/githubnext/ado-aw/issues/298)) ([1066a2f](https://github.com/githubnext/ado-aw/commit/1066a2fee0c30991a50bf6a50b221308b43ee6a8))
* align engine front matter with gh-aw and hook up Engine enum ([#286](https://github.com/githubnext/ado-aw/issues/286)) ([f90bd81](https://github.com/githubnext/ado-aw/commit/f90bd81c5e6eb83bdf595332a493da41b29e4fea))
* remove `run` subcommand ([#306](https://github.com/githubnext/ado-aw/issues/306)) ([a71ed16](https://github.com/githubnext/ado-aw/commit/a71ed16db5a472e7727c0f6637cfd88b7097eef8))


### Bug Fixes

* prefix agent filename with ado to distinguish from gh-aw ([#299](https://github.com/githubnext/ado-aw/issues/299)) ([b3ecca7](https://github.com/githubnext/ado-aw/commit/b3ecca7ee241d4798ce91e815cf281d28f4e15cd))

## [0.16.0](https://github.com/githubnext/ado-aw/compare/v0.15.0...v0.16.0) (2026-04-21)


### Features

* **check:** surface specific violations with unified diff output ([#278](https://github.com/githubnext/ado-aw/issues/278)) ([57618e8](https://github.com/githubnext/ado-aw/commit/57618e8110a57507e1f6f925d819c508ea8297ce))


### Bug Fixes

* align tool allow lists with gh-aw ([#279](https://github.com/githubnext/ado-aw/issues/279)) ([be3b4c5](https://github.com/githubnext/ado-aw/commit/be3b4c5a128d1f06facfea6bb3cde03d65a97b1d))
* revert pipeline prompt to $(cat) inline expansion ([#276](https://github.com/githubnext/ado-aw/issues/276)) ([784de06](https://github.com/githubnext/ado-aw/commit/784de069a46be9dd976033a383422c0f5ef61052))

## [0.15.0](https://github.com/githubnext/ado-aw/compare/v0.14.0...v0.15.0) (2026-04-21)


### Features

* add /scout issue slash-command workflow ([#258](https://github.com/githubnext/ado-aw/issues/258)) ([57afa1e](https://github.com/githubnext/ado-aw/commit/57afa1e9c947476a59184bb01bf12d26a642c94c))
* add ado-aw run subcommand for local development ([#266](https://github.com/githubnext/ado-aw/issues/266)) ([55db04c](https://github.com/githubnext/ado-aw/commit/55db04cb8cd77e2938977b523d9d2a868b2b8bf2))
* add macOS x64 and arm64 release binaries in GitHub Actions ([#264](https://github.com/githubnext/ado-aw/issues/264)) ([9bfe8ea](https://github.com/githubnext/ado-aw/commit/9bfe8ea79d8f224f4586c2ff83d957e5ed536e0d))
* **compile:** add --skip-integrity flag for debug builds ([#246](https://github.com/githubnext/ado-aw/issues/246)) ([8cae693](https://github.com/githubnext/ado-aw/commit/8cae693d2e5b4c51cd717e9b4ecb5affa99b3824))
* **execute:** add --dry-run flag to execute command ([#265](https://github.com/githubnext/ado-aw/issues/265)) ([b2955d4](https://github.com/githubnext/ado-aw/commit/b2955d45e8bc31e884fd76db7c6bb30544d6227c))
* model GitHub and SafeOutputs as extensions, add --allow-tool for all MCP servers ([#251](https://github.com/githubnext/ado-aw/issues/251)) ([2150cdb](https://github.com/githubnext/ado-aw/commit/2150cdb560dcbd41aef924248ebbb83a84cec0fb))


### Bug Fixes

* add required 'domain' field to MCPG gateway config ([#249](https://github.com/githubnext/ado-aw/issues/249)) ([e355b92](https://github.com/githubnext/ado-aw/commit/e355b921cb8819882d8fea7df886b0ebd0b29813))
* align MCPG integration with gh-aw reference implementation ([#244](https://github.com/githubnext/ado-aw/issues/244)) ([bb4cccd](https://github.com/githubnext/ado-aw/commit/bb4cccddafe48914ae09bd3e78ca440e7ee70fc9))
* bypass MCPG run_containerized.sh to fix port validation with --network host ([#250](https://github.com/githubnext/ado-aw/issues/250)) ([9b0568e](https://github.com/githubnext/ado-aw/commit/9b0568ea5959eecbb686057085b1fa60643ac1e6))
* resolve compiler warnings and remove dead code ([#268](https://github.com/githubnext/ado-aw/issues/268)) ([dd7df1f](https://github.com/githubnext/ado-aw/commit/dd7df1ff9bc68cdcd0fe725732d872b7c793f18f))
* **run:** base64-encode PAT for ADO MCP and pass prompts via [@file](https://github.com/file) ([#270](https://github.com/githubnext/ado-aw/issues/270)) ([11b01ec](https://github.com/githubnext/ado-aw/commit/11b01ec207f050d324488f87894fbf0eae7675be))

## [0.14.0](https://github.com/githubnext/ado-aw/compare/v0.13.0...v0.14.0) (2026-04-16)


### Features

* unify standalone and 1ES compilers ([#226](https://github.com/githubnext/ado-aw/issues/226)) ([1e396a0](https://github.com/githubnext/ado-aw/commit/1e396a0d9c9c0d8f2e9e2b87c8de613c10dc0822))


### Bug Fixes

* remove legacy service-connection MCP field ([#233](https://github.com/githubnext/ado-aw/issues/233)) ([#234](https://github.com/githubnext/ado-aw/issues/234)) ([baf5fe9](https://github.com/githubnext/ado-aw/commit/baf5fe9d95aea452a342620027cda7c861c8e346))
* rename resolve-pr-review-thread to resolve-pr-thread ([#228](https://github.com/githubnext/ado-aw/issues/228)) ([#231](https://github.com/githubnext/ado-aw/issues/231)) ([e1d3735](https://github.com/githubnext/ado-aw/commit/e1d37350b191791dbb02615a93b5431c722499ee))

## [0.13.0](https://github.com/githubnext/ado-aw/compare/v0.12.0...v0.13.0) (2026-04-15)


### Features

* capture agent statistics via OTel and surface in safe outputs ([#219](https://github.com/githubnext/ado-aw/issues/219)) ([6f8d0f7](https://github.com/githubnext/ado-aw/commit/6f8d0f7069907b093a024b0f89b45846b292cb7b))

## [0.12.0](https://github.com/githubnext/ado-aw/compare/v0.11.0...v0.12.0) (2026-04-15)


### Features

* add ecosystem domain allowlists from gh-aw ([#213](https://github.com/githubnext/ado-aw/issues/213)) ([22a2069](https://github.com/githubnext/ado-aw/commit/22a2069d49b96774acbd1ec42323cb7c7092e4b0))
* add Lean 4 runtime support with runtimes: front matter ([#208](https://github.com/githubnext/ado-aw/issues/208)) ([16fc9be](https://github.com/githubnext/ado-aw/commit/16fc9be54519f0371422afc0d6d59e6f93463f09))
* standardise front matter sanitization via SanitizeConfig/SanitizeContent traits ([#210](https://github.com/githubnext/ado-aw/issues/210)) ([85ac3ab](https://github.com/githubnext/ado-aw/commit/85ac3ab38ec8d696cf28a558ed3e0a9473e405b7))


### Bug Fixes

* **create-pr:** skip auto-complete API call on draft PRs ([#194](https://github.com/githubnext/ado-aw/issues/194)) ([#200](https://github.com/githubnext/ado-aw/issues/200)) ([d58dbeb](https://github.com/githubnext/ado-aw/commit/d58dbeb8337e4d11ddf6fb39a87511bcb3df327c))
* improve trigger.pipeline validation with expression checks and better errors ([#189](https://github.com/githubnext/ado-aw/issues/189), [#188](https://github.com/githubnext/ado-aw/issues/188)) ([#196](https://github.com/githubnext/ado-aw/issues/196)) ([b0ae590](https://github.com/githubnext/ado-aw/commit/b0ae590bc4f39bcb9efaa7db5c340c86b15043e5))

## [0.11.0](https://github.com/githubnext/ado-aw/compare/v0.10.0...v0.11.0) (2026-04-14)


### ⚠ BREAKING CHANGES

* The create subcommand has been removed. Use ado-aw init instead.

### Features

* **create-pr:** align with gh-aw create-pull-request implementation ([#155](https://github.com/githubnext/ado-aw/issues/155)) ([b1859ae](https://github.com/githubnext/ado-aw/commit/b1859ae92934a12ff6d305d06804929cff96bf0b))
* replace create wizard with AI-first onboarding ([#187](https://github.com/githubnext/ado-aw/issues/187)) ([c468320](https://github.com/githubnext/ado-aw/commit/c468320d868b943826129d08b17a0c3ad5ca805c))


### Bug Fixes

* address injection vulnerabilities from red team audit ([#171](https://github.com/githubnext/ado-aw/issues/171)) ([#175](https://github.com/githubnext/ado-aw/issues/175)) ([5e3ac1b](https://github.com/githubnext/ado-aw/commit/5e3ac1bcba42da9f640479fd4bd77766db97c5de))
* pin prompt URLs to version tag instead of main branch ([#191](https://github.com/githubnext/ado-aw/issues/191)) ([1f3b6da](https://github.com/githubnext/ado-aw/commit/1f3b6da140edba1b527d79c5a20560abe5fb923a))

## [0.10.0](https://github.com/githubnext/ado-aw/compare/v0.9.0...v0.10.0) (2026-04-14)


### Features

* add red team security auditor agentic workflow ([#170](https://github.com/githubnext/ado-aw/issues/170)) ([6700677](https://github.com/githubnext/ado-aw/commit/67006771abd5119c9bf8109c87110a91f4406f63))
* add runtime parameters support with auto-injected clearMemory ([#166](https://github.com/githubnext/ado-aw/issues/166)) ([dc5b766](https://github.com/githubnext/ado-aw/commit/dc5b76654f59557092201289d0b9d2bfd5613536))
* align MCP config with MCPG spec — container/HTTP transport ([#157](https://github.com/githubnext/ado-aw/issues/157)) ([a46a85b](https://github.com/githubnext/ado-aw/commit/a46a85b2c9870e7bd4bd36f99e6ff1cbb06c7ad7))
* enable real-time agent output streaming with VSO filtering ([#159](https://github.com/githubnext/ado-aw/issues/159)) ([7497cd6](https://github.com/githubnext/ado-aw/commit/7497cd6bf243b781137c3994ba7187b34331bcb3))
* **mcp:** filter SafeOutputs tools based on front matter config ([#156](https://github.com/githubnext/ado-aw/issues/156)) ([f43b22e](https://github.com/githubnext/ado-aw/commit/f43b22ed1228b98b7bc9085494b275681e06a265))
* promote memory to cache-memory tool and add first-class azure-devops tool ([#167](https://github.com/githubnext/ado-aw/issues/167)) ([39103e1](https://github.com/githubnext/ado-aw/commit/39103e1237fdeacfd6bc1b39bac62e493cb7848c))
* swap to using aw-mcpg ([#19](https://github.com/githubnext/ado-aw/issues/19)) ([1cddfb3](https://github.com/githubnext/ado-aw/commit/1cddfb3e6001736c5cd1cc0ce572521370001e1c))


### Bug Fixes

* address review findings — MCPG_IMAGE constant, constant-time auth, reqwest dev-dep, MCP enabled check ([#151](https://github.com/githubnext/ado-aw/issues/151)) ([09be1f2](https://github.com/githubnext/ado-aw/commit/09be1f213152f151da44efece2cd114e52cf6fc4))
* use length-check + ct_eq for constant-time auth comparison ([#153](https://github.com/githubnext/ado-aw/issues/153)) ([d0aed74](https://github.com/githubnext/ado-aw/commit/d0aed7422f3a473491392a4198158f1ea03f56e3))

## [0.9.0](https://github.com/githubnext/ado-aw/compare/v0.8.3...v0.9.0) (2026-04-11)


### Features

* add COPILOT_CLI_VERSION to dependency version updater workflow ([#137](https://github.com/githubnext/ado-aw/issues/137)) ([8155d24](https://github.com/githubnext/ado-aw/commit/8155d240522658805fed12210d17df5c2a943b5a))
* map engine max-turns and timeout-minutes to Copilot CLI arguments ([#134](https://github.com/githubnext/ado-aw/issues/134)) ([2dbe162](https://github.com/githubnext/ado-aw/commit/2dbe1629400ed358199e94b5907e66b1ac221dec))


### Bug Fixes

* deprecate max-turns and move timeout-minutes to YAML job property ([#138](https://github.com/githubnext/ado-aw/issues/138)) ([9887c97](https://github.com/githubnext/ado-aw/commit/9887c9794d732d76eade8fb1b33cfb99cb02522a))
* report-incomplete fails pipeline, percent-encode user_id, stage-1 status validation, merge_strategy validation, dead code removal ([#141](https://github.com/githubnext/ado-aw/issues/141)) ([e81c570](https://github.com/githubnext/ado-aw/commit/e81c5707baf9572e31de3fef21de232bb60bf796))

## [0.8.3](https://github.com/githubnext/ado-aw/compare/v0.8.2...v0.8.3) (2026-04-02)


### Bug Fixes

* handle string WikiType enum from ADO API in branch resolution ([#122](https://github.com/githubnext/ado-aw/issues/122)) ([7914169](https://github.com/githubnext/ado-aw/commit/791416976e805e440fe60a5829f79ebca2143cdb))
* resolve check command source path from repo root ([#120](https://github.com/githubnext/ado-aw/issues/120)) ([a057598](https://github.com/githubnext/ado-aw/commit/a05759895352aa6504fa62ab74bde3ee62a9e25e))

## [0.8.2](https://github.com/githubnext/ado-aw/compare/v0.8.1...v0.8.2) (2026-04-02)


### Bug Fixes

* use platform-appropriate absolute paths in path fallback tests ([#118](https://github.com/githubnext/ado-aw/issues/118)) ([434cf19](https://github.com/githubnext/ado-aw/commit/434cf19562563199e3114fbaaa04b3d3c33aad98))

## [0.8.1](https://github.com/githubnext/ado-aw/compare/v0.8.0...v0.8.1) (2026-04-02)


### Bug Fixes

* auto-detect code wiki branch for wiki page safe outputs ([#115](https://github.com/githubnext/ado-aw/issues/115)) ([f8ea1e9](https://github.com/githubnext/ado-aw/commit/f8ea1e9b2c44ca7cce02398fa6a7fd2d20edf252))
* preserve subdirectory in generated pipeline_path and source_path ([#114](https://github.com/githubnext/ado-aw/issues/114)) ([32137fe](https://github.com/githubnext/ado-aw/commit/32137fe1c3169ee7209fa71368abcbf9165ddc36))

## [0.8.0](https://github.com/githubnext/ado-aw/compare/ado-aw-v0.7.1...ado-aw-v0.8.0) (2026-04-01)


### ⚠ BREAKING CHANGES

* \check <source> <pipeline>\ is now \check <pipeline>\. Update any scripts or pipeline templates that call the old two-arg form.

### Features

* add /rust-review slash command for on-demand PR reviews ([#60](https://github.com/githubnext/ado-aw/issues/60)) ([8eaf972](https://github.com/githubnext/ado-aw/commit/8eaf972baed99bcd03f9d3bcae013bdb922e390a))
* add \configure\ command to detect pipelines and update GITHUB_TOKEN ([#92](https://github.com/githubnext/ado-aw/issues/92)) ([a032b4e](https://github.com/githubnext/ado-aw/commit/a032b4e837df89fe5127100664f44415274f1030))
* add comment-on-work-item safe output tool ([#80](https://github.com/githubnext/ado-aw/issues/80)) ([513f7fe](https://github.com/githubnext/ado-aw/commit/513f7feca82e4d6c2ae02906ea6231fa2ebf4530))
* add create-wiki-page safe output ([#61](https://github.com/githubnext/ado-aw/issues/61)) ([87d6527](https://github.com/githubnext/ado-aw/commit/87d65276a084b5ab944f33083089cb2a7fe93434))
* add edit-wiki-page safe output ([#58](https://github.com/githubnext/ado-aw/issues/58)) ([7b4536f](https://github.com/githubnext/ado-aw/commit/7b4536f1953c9d09bb3deee5a06779c06e4ac53e))
* add update-work-item safe output ([#65](https://github.com/githubnext/ado-aw/issues/65)) ([cf5e6b5](https://github.com/githubnext/ado-aw/commit/cf5e6b5a0778dda1cfcbfeb0d5a19d89649ce43a))
* Add Windows x64 binary to release artifacts ([#37](https://github.com/githubnext/ado-aw/issues/37)) ([d463006](https://github.com/githubnext/ado-aw/commit/d4630063c6f6fb8418fe0e37c3ef56abed1fa299))
* allow copilot bot to trigger rust PR reviewer ([#59](https://github.com/githubnext/ado-aw/issues/59)) ([0bcef57](https://github.com/githubnext/ado-aw/commit/0bcef5799295157f6d1f3cda06de4c7fcd020730))
* apply max budget enforcement to all safe-output tools ([#91](https://github.com/githubnext/ado-aw/issues/91)) ([e88d8da](https://github.com/githubnext/ado-aw/commit/e88d8da4e8370547b5a22b623c850287402abab6))
* auto-detect source from header in check command ([#108](https://github.com/githubnext/ado-aw/issues/108)) ([b25f143](https://github.com/githubnext/ado-aw/commit/b25f1431928543af23a471210f2c4e8422e9d86e))
* auto-discover and recompile all agentic pipelines ([#96](https://github.com/githubnext/ado-aw/issues/96)) ([fb1de50](https://github.com/githubnext/ado-aw/commit/fb1de50f6ac89d13a1876f8c5374c933139345ee))
* **configure:** accept explicit definition IDs via --definition-ids ([#100](https://github.com/githubnext/ado-aw/issues/100)) ([b12c5ff](https://github.com/githubnext/ado-aw/commit/b12c5ffb493479976b555b115f805eca63e0c967))
* Download releases from GitHub. ([#17](https://github.com/githubnext/ado-aw/issues/17)) ([8478453](https://github.com/githubnext/ado-aw/commit/847845351026c7683f5f852ac06c084c2c2fe00f))
* rename edit-wiki-page to update-wiki-page ([#66](https://github.com/githubnext/ado-aw/issues/66)) ([2b6c5ed](https://github.com/githubnext/ado-aw/commit/2b6c5ed5bbfd5874231adaac3d27f45ac0c1d3f1))
* replace read-only-service-connection with permissions field ([#26](https://github.com/githubnext/ado-aw/issues/26)) ([410e2df](https://github.com/githubnext/ado-aw/commit/410e2dff48c56dd3e66773e7c2f6cb6295eb9055))


### Bug Fixes

* add --repo flag to gh release upload in checksums job ([#40](https://github.com/githubnext/ado-aw/issues/40)) ([fd437da](https://github.com/githubnext/ado-aw/commit/fd437daf148e63f2c768fd9c6365c5c8ac4ef871))
* **configure:** support Azure CLI auth and fix YAML path matching ([#98](https://github.com/githubnext/ado-aw/issues/98)) ([a771036](https://github.com/githubnext/ado-aw/commit/a771036b21849f5769bc3d44cb72346a93d73bac))
* pin AWF container images to specific firewall version ([#30](https://github.com/githubnext/ado-aw/issues/30)) ([bb92c9c](https://github.com/githubnext/ado-aw/commit/bb92c9ccc6b5edbfa6b0ddeabca1cbe0cd39dd98))
* pin AWF container images to specific firewall version ([#32](https://github.com/githubnext/ado-aw/issues/32)) ([9c3b85c](https://github.com/githubnext/ado-aw/commit/9c3b85c3029a513f75dc354be3b6052098cd43db))
* pin DockerInstaller to v26.1.4 for API compatibility ([#105](https://github.com/githubnext/ado-aw/issues/105)) ([2c6baf2](https://github.com/githubnext/ado-aw/commit/2c6baf28766981ec92d2129def1060f821b0393d))
* quote chmod paths and remove fragile sha256sum pipe in templates ([#43](https://github.com/githubnext/ado-aw/issues/43)) ([b246fb1](https://github.com/githubnext/ado-aw/commit/b246fb157177994224eb4c4a8930f11148a653c3))
* sha256sum --ignore-missing silently passes when binary is absent from checksums.txt ([#47](https://github.com/githubnext/ado-aw/issues/47)) ([26c03c4](https://github.com/githubnext/ado-aw/commit/26c03c4e1c9c0ffde1e1570a70fbe90744c9383f))
* strip redundant ./ prefixes from source path in header comment ([#106](https://github.com/githubnext/ado-aw/issues/106)) ([689825c](https://github.com/githubnext/ado-aw/commit/689825c6ab073864e8b380e3ee86ec3c2d120f45))
* **tests:** strengthen checksum verification assertion against regression ([#48](https://github.com/githubnext/ado-aw/issues/48)) ([7fcabe2](https://github.com/githubnext/ado-aw/commit/7fcabe2b4494dd694127d4f11ed7aad98b6b21e9))
* update Copilot CLI version to 1.0.6 via compiler constants ([#51](https://github.com/githubnext/ado-aw/issues/51)) ([b8d8ece](https://github.com/githubnext/ado-aw/commit/b8d8ece8777b1517a6d8ced9c0c89f85ac088932))
* YAML path matching and legacy SSH URL support ([#95](https://github.com/githubnext/ado-aw/issues/95)) ([f85dd39](https://github.com/githubnext/ado-aw/commit/f85dd39b5038be49ce6ed0f67d34d170090999e4))

## [0.7.1](https://github.com/githubnext/ado-aw/compare/v0.7.0...v0.7.1) (2026-04-01)


### Bug Fixes

* pin DockerInstaller to v26.1.4 for API compatibility ([#105](https://github.com/githubnext/ado-aw/issues/105)) ([2c6baf2](https://github.com/githubnext/ado-aw/commit/2c6baf28766981ec92d2129def1060f821b0393d))
* strip redundant ./ prefixes from source path in header comment ([#106](https://github.com/githubnext/ado-aw/issues/106)) ([689825c](https://github.com/githubnext/ado-aw/commit/689825c6ab073864e8b380e3ee86ec3c2d120f45))

## [0.7.0](https://github.com/githubnext/ado-aw/compare/v0.6.1...v0.7.0) (2026-03-31)


### Features

* **configure:** accept explicit definition IDs via --definition-ids ([#100](https://github.com/githubnext/ado-aw/issues/100)) ([b12c5ff](https://github.com/githubnext/ado-aw/commit/b12c5ffb493479976b555b115f805eca63e0c967))

## [0.6.1](https://github.com/githubnext/ado-aw/compare/v0.6.0...v0.6.1) (2026-03-31)


### Bug Fixes

* **configure:** support Azure CLI auth and fix YAML path matching ([#98](https://github.com/githubnext/ado-aw/issues/98)) ([a771036](https://github.com/githubnext/ado-aw/commit/a771036b21849f5769bc3d44cb72346a93d73bac))

## [0.6.0](https://github.com/githubnext/ado-aw/compare/v0.5.0...v0.6.0) (2026-03-31)


### Features

* auto-discover and recompile all agentic pipelines ([#96](https://github.com/githubnext/ado-aw/issues/96)) ([fb1de50](https://github.com/githubnext/ado-aw/commit/fb1de50f6ac89d13a1876f8c5374c933139345ee))


### Bug Fixes

* YAML path matching and legacy SSH URL support ([#95](https://github.com/githubnext/ado-aw/issues/95)) ([f85dd39](https://github.com/githubnext/ado-aw/commit/f85dd39b5038be49ce6ed0f67d34d170090999e4))

## [0.5.0](https://github.com/githubnext/ado-aw/compare/v0.4.0...v0.5.0) (2026-03-31)


### Features

* add \configure\ command to detect pipelines and update GITHUB_TOKEN ([#92](https://github.com/githubnext/ado-aw/issues/92)) ([a032b4e](https://github.com/githubnext/ado-aw/commit/a032b4e837df89fe5127100664f44415274f1030))

## [0.4.0](https://github.com/githubnext/ado-aw/compare/v0.3.2...v0.4.0) (2026-03-30)


### Features

* add /rust-review slash command for on-demand PR reviews ([#60](https://github.com/githubnext/ado-aw/issues/60)) ([8eaf972](https://github.com/githubnext/ado-aw/commit/8eaf972baed99bcd03f9d3bcae013bdb922e390a))
* add comment-on-work-item safe output tool ([#80](https://github.com/githubnext/ado-aw/issues/80)) ([513f7fe](https://github.com/githubnext/ado-aw/commit/513f7feca82e4d6c2ae02906ea6231fa2ebf4530))
* add create-wiki-page safe output ([#61](https://github.com/githubnext/ado-aw/issues/61)) ([87d6527](https://github.com/githubnext/ado-aw/commit/87d65276a084b5ab944f33083089cb2a7fe93434))
* add edit-wiki-page safe output ([#58](https://github.com/githubnext/ado-aw/issues/58)) ([7b4536f](https://github.com/githubnext/ado-aw/commit/7b4536f1953c9d09bb3deee5a06779c06e4ac53e))
* add update-work-item safe output ([#65](https://github.com/githubnext/ado-aw/issues/65)) ([cf5e6b5](https://github.com/githubnext/ado-aw/commit/cf5e6b5a0778dda1cfcbfeb0d5a19d89649ce43a))
* allow copilot bot to trigger rust PR reviewer ([#59](https://github.com/githubnext/ado-aw/issues/59)) ([0bcef57](https://github.com/githubnext/ado-aw/commit/0bcef5799295157f6d1f3cda06de4c7fcd020730))
* apply max budget enforcement to all safe-output tools ([#91](https://github.com/githubnext/ado-aw/issues/91)) ([e88d8da](https://github.com/githubnext/ado-aw/commit/e88d8da4e8370547b5a22b623c850287402abab6))
* rename edit-wiki-page to update-wiki-page ([#66](https://github.com/githubnext/ado-aw/issues/66)) ([2b6c5ed](https://github.com/githubnext/ado-aw/commit/2b6c5ed5bbfd5874231adaac3d27f45ac0c1d3f1))


### Bug Fixes

* sha256sum --ignore-missing silently passes when binary is absent from checksums.txt ([#47](https://github.com/githubnext/ado-aw/issues/47)) ([26c03c4](https://github.com/githubnext/ado-aw/commit/26c03c4e1c9c0ffde1e1570a70fbe90744c9383f))
* **tests:** strengthen checksum verification assertion against regression ([#48](https://github.com/githubnext/ado-aw/issues/48)) ([7fcabe2](https://github.com/githubnext/ado-aw/commit/7fcabe2b4494dd694127d4f11ed7aad98b6b21e9))
* update Copilot CLI version to 1.0.6 via compiler constants ([#51](https://github.com/githubnext/ado-aw/issues/51)) ([b8d8ece](https://github.com/githubnext/ado-aw/commit/b8d8ece8777b1517a6d8ced9c0c89f85ac088932))

## [0.3.2](https://github.com/githubnext/ado-aw/compare/v0.3.1...v0.3.2) (2026-03-17)


### Bug Fixes

* quote chmod paths and remove fragile sha256sum pipe in templates ([#43](https://github.com/githubnext/ado-aw/issues/43)) ([b246fb1](https://github.com/githubnext/ado-aw/commit/b246fb157177994224eb4c4a8930f11148a653c3))

## [0.3.1](https://github.com/githubnext/ado-aw/compare/v0.3.0...v0.3.1) (2026-03-17)


### Bug Fixes

* add --repo flag to gh release upload in checksums job ([#40](https://github.com/githubnext/ado-aw/issues/40)) ([fd437da](https://github.com/githubnext/ado-aw/commit/fd437daf148e63f2c768fd9c6365c5c8ac4ef871))

## [0.3.0](https://github.com/githubnext/ado-aw/compare/v0.2.0...v0.3.0) (2026-03-17)


### Features

* Add Windows x64 binary to release artifacts ([#37](https://github.com/githubnext/ado-aw/issues/37)) ([d463006](https://github.com/githubnext/ado-aw/commit/d4630063c6f6fb8418fe0e37c3ef56abed1fa299))
* Download releases from GitHub. ([#17](https://github.com/githubnext/ado-aw/issues/17)) ([8478453](https://github.com/githubnext/ado-aw/commit/847845351026c7683f5f852ac06c084c2c2fe00f))
* replace read-only-service-connection with permissions field ([#26](https://github.com/githubnext/ado-aw/issues/26)) ([410e2df](https://github.com/githubnext/ado-aw/commit/410e2dff48c56dd3e66773e7c2f6cb6295eb9055))


### Bug Fixes

* pin AWF container images to specific firewall version ([#30](https://github.com/githubnext/ado-aw/issues/30)) ([bb92c9c](https://github.com/githubnext/ado-aw/commit/bb92c9ccc6b5edbfa6b0ddeabca1cbe0cd39dd98))
* pin AWF container images to specific firewall version ([#32](https://github.com/githubnext/ado-aw/issues/32)) ([9c3b85c](https://github.com/githubnext/ado-aw/commit/9c3b85c3029a513f75dc354be3b6052098cd43db))

## [0.2.0](https://github.com/githubnext/ado-aw/compare/v0.1.3...v0.2.0) (2026-03-17)


### Features

* Add Windows x64 binary to release artifacts ([#37](https://github.com/githubnext/ado-aw/issues/37)) ([d463006](https://github.com/githubnext/ado-aw/commit/d4630063c6f6fb8418fe0e37c3ef56abed1fa299))

## [0.1.3](https://github.com/githubnext/ado-aw/compare/v0.1.2...v0.1.3) (2026-03-16)


### Bug Fixes

* pin AWF container images to specific firewall version ([#32](https://github.com/githubnext/ado-aw/issues/32)) ([9c3b85c](https://github.com/githubnext/ado-aw/commit/9c3b85c3029a513f75dc354be3b6052098cd43db))

## [0.1.2](https://github.com/githubnext/ado-aw/compare/v0.1.1...v0.1.2) (2026-03-16)


### Bug Fixes

* pin AWF container images to specific firewall version ([#30](https://github.com/githubnext/ado-aw/issues/30)) ([bb92c9c](https://github.com/githubnext/ado-aw/commit/bb92c9ccc6b5edbfa6b0ddeabca1cbe0cd39dd98))

## 0.1.0 (2026-03-13)


### Features

* Download releases from GitHub. ([#17](https://github.com/githubnext/ado-aw/issues/17)) ([8478453](https://github.com/githubnext/ado-aw/commit/847845351026c7683f5f852ac06c084c2c2fe00f))
* replace read-only-service-connection with permissions field ([#26](https://github.com/githubnext/ado-aw/issues/26)) ([410e2df](https://github.com/githubnext/ado-aw/commit/410e2dff48c56dd3e66773e7c2f6cb6295eb9055))
