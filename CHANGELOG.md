# Changelog

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
