# Changelog

## [0.4.0](https://github.com/AI-Substrate/umx/compare/v0.3.0...v0.4.0) (2026-11-23)


### ⚠ BREAKING CHANGES

* neighbours lists the edges of the node's entity members
* ontology adopt-std --field-to-tag moves a field into std tags
* tag filters on find, search, walk and neighbours, and IN n.tags
* std tags and the node list tag filter

### Features

* doctor hints adopt-std for a graph made without std; the skill writes std names ([d2500ac](https://github.com/AI-Substrate/umx/commit/d2500aced5646fe6bc4d40826e4da6bfe24cdb9e))
* neighbours lists the edges of the node's entity members ([7127cb5](https://github.com/AI-Substrate/umx/commit/7127cb5c5611c340a09a4b59e70411c4445e3228))
* node list --columns spine:&lt;name&gt; gives each node's place on a declared spine ([fe6a5b4](https://github.com/AI-Substrate/umx/commit/fe6a5b4c6a3e8716dd4d73770884f4c43733e4cc))
* ontology adopt-std --field-to-tag moves a field into std tags ([6c3ff84](https://github.com/AI-Substrate/umx/commit/6c3ff849d64b99a01d8220591048a7d59bedb4d8))
* std tags and the node list tag filter ([f3ed406](https://github.com/AI-Substrate/umx/commit/f3ed406843d4ec9a3bdbdd323494e4697c378c8f))
* tag filters on find, search, walk and neighbours, and IN n.tags ([0c87dcf](https://github.com/AI-Substrate/umx/commit/0c87dcfed2eaff7b4daa4a9020712847e49d8785))
* **ui:** a reload keeps the open graphs and the view's look ([e1c8aa5](https://github.com/AI-Substrate/umx/commit/e1c8aa59db0c2fd461031acfcce91ab1fe87e075))
* **ui:** choose graphs in a searchable dialog, down to none ([4baf7fb](https://github.com/AI-Substrate/umx/commit/4baf7fb7965d18af400619a73dc5fa3d29498981))
* **ui:** lay a graph out along its spine ([f8805f7](https://github.com/AI-Substrate/umx/commit/f8805f7f74e05f19e3db6934502f49a242ab7bd3))


### Bug Fixes

* node get and node list --columns locator show the whole locator ([69e8142](https://github.com/AI-Substrate/umx/commit/69e8142c3af725cf82ba9bd0133ddd88f8df3c71))
* **ui:** keep spine picks in the tab, never in the fragment ([ae77614](https://github.com/AI-Substrate/umx/commit/ae776149c7be7853ecb0c0fbe19cd6b05a4987db))
* **ui:** the graph dialog closes itself and gives focus back; document what a reload keeps ([9c98e17](https://github.com/AI-Substrate/umx/commit/9c98e17fcf8f0913d90f1e1eace5eee334e2af74))
* **ui:** the graph dialog's count reads as selected, up to the limit ([83b69fa](https://github.com/AI-Substrate/umx/commit/83b69fa2f8c84359c33f9d8eb2e74ce920dd55a9))

## [0.3.0](https://github.com/AI-Substrate/umx/compare/v0.2.0...v0.3.0) (2026-11-19)


### ⚠ BREAKING CHANGES

* ontology adopt-std moves a graph onto std in one write set (req-101)
* umx search takes up to 5 phrasings with --also (search-09)
* protocol 92. `GraphConfirmPolicy` loses `typed`, and the CLI's /dev/tty prompt is gone. The error `policy_confirm_needs_human` is retired. `policy_downgrade_unconfirmed`'s next is now `umx graph confirm-policy <alias>`. `similarity_no_embedder` no longer prints a command that lowers a policy; it points to adding a local instance and calls hosted instances the owner's decision.
* std reserved names, clashes and soft endpoint types (req-99)

### Features

* custom fields on std types (req-100) ([2e866ac](https://github.com/AI-Substrate/umx/commit/2e866ac59708d7c219df29f335b1ada45455e3f9))
* install.ps1 installs umx inside WSL 2 on Windows ([7b902bc](https://github.com/AI-Substrate/umx/commit/7b902bc0c2128dd54af1adde58eca203e855c7df))
* lower a graph's embedding policy in one call, warned and recorded ([a7dd1f2](https://github.com/AI-Substrate/umx/commit/a7dd1f2d7e6a57207e6281dd5c1650b72b7fb0bb))
* ontology adopt-std moves a graph onto std in one write set (req-101) ([5485a36](https://github.com/AI-Substrate/umx/commit/5485a36da33d5cac8607eb4ec22e562a98a0f2d4))
* std reserved names, clashes and soft endpoint types (req-99) ([f10e18f](https://github.com/AI-Substrate/umx/commit/f10e18ff0b9d4f5dc9ae6f8e031718aa64bdfe4e))
* umx search takes up to 5 phrasings with --also (search-09) ([0222cfc](https://github.com/AI-Substrate/umx/commit/0222cfcaf1c42b18b2a911fd66e7328e5d364bf3))


### Bug Fixes

* a pinned install.ps1 runs that release's install.sh, checked against its SHA256SUMS ([a1e531a](https://github.com/AI-Substrate/umx/commit/a1e531aef955163eb34d8e4d8ae0a6a7863673d4))
* release builds say when they were built ([985d71f](https://github.com/AI-Substrate/umx/commit/985d71f41682fe9d60f9ca92196a6f1ea9e99aed))


### Performance

* background embedding fills each provider call up to the instance's own limit ([45ff623](https://github.com/AI-Substrate/umx/commit/45ff6237a2c1205841192042719035b06be4fce9))


### Documentation

* the attestation checks name the release workflow ([82d3fd0](https://github.com/AI-Substrate/umx/commit/82d3fd09d63b0d2a4c8c8964597148f15b61692a))

## [0.2.0](https://github.com/AI-Substrate/umx/compare/v0.1.0...v0.2.0) (2026-11-19)


### ⚠ BREAKING CHANGES

* std graphs and the effective ontology (req-98)

### Features

* std graphs and the effective ontology (req-98) ([dbd7334](https://github.com/AI-Substrate/umx/commit/dbd7334063bbf41bfaa052ee55278a11c640c01a))


### Bug Fixes

* a busy daemon is reported busy on Linux; flaky daemon, ui and cli tests made deterministic ([1e1b8b5](https://github.com/AI-Substrate/umx/commit/1e1b8b589f2cb8aba8f5998f4907f7c4cb6ea36c))
* **ui:** the lock age check reads a package name with at most one slash ([38cb291](https://github.com/AI-Substrate/umx/commit/38cb2912c823314d9820fd686a27eb66c0428b52))


### Documentation

* install umx from a release; the README starts with the agent block ([f368ebf](https://github.com/AI-Substrate/umx/commit/f368ebfeb32e9aa62ecb7d4acbc928dadbd70ec2))

## 0.1.0 (2026-11-18)


### Features

* install.sh installs a verified umx release on macOS and Linux ([bec3abd](https://github.com/AI-Substrate/umx/commit/bec3abd59e0f04fe098ac869e2b9108c3310a8b0))
* version umx with release-please from 0.1.0 ([14393e1](https://github.com/AI-Substrate/umx/commit/14393e14e28b4a6854dde5ad89a1fd615d42237b))
