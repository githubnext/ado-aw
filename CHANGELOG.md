# Changelog

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
