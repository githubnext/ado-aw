# Changelog

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
