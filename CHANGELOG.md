# Changelog

## [0.6.1](https://github.com/AI-Substrate/umx/compare/v0.6.0...v0.6.1) (2026-11-26)


### Features

* add graph-local rule sets and risk ownership advice ([8aa1af7](https://github.com/AI-Substrate/umx/commit/8aa1af7a4bf5d9756252ee60c25cb514af87b263))


### Bug Fixes

* hub follow-ups ([942bcc6](https://github.com/AI-Substrate/umx/commit/942bcc60afdcfa99de2dda922fb3ab9c04e23966))
* version non-std rule switches and guide snapshot reloads ([1d3ae32](https://github.com/AI-Substrate/umx/commit/1d3ae32827efb1dee47fc8b34aef276d3e07fe92))


### Documentation

* PRD 003, a hub whose node is retired or gone says so ([2618a39](https://github.com/AI-Substrate/umx/commit/2618a397cb044fa3f6f4a2b5d503036c3e1b47c8))
* rules and hubs read end to end; the skill sets meetings up as hubs on the meeting spine ([3465eb7](https://github.com/AI-Substrate/umx/commit/3465eb7c74f79be8ad4a37ba4fcf7bd58168e14f))
* set the meeting spine as main only when the graph has none ([4cab734](https://github.com/AI-Substrate/umx/commit/4cab7343d8ed40bdfba0bc059ef4961b2f6bbe27))

## [0.6.0](https://github.com/AI-Substrate/umx/compare/v0.5.2...v0.6.0) (2026-11-26)


### ⚠ BREAKING CHANGES

* plain umx check warns about changed and missing cited files
* label search hits strong, fair or weak against the nodes searched (search-10)
* the daemon answers at once and reloads remembered graphs behind it
* hash a cited source when the reference is added

### Features

* add bounded graph rule management ([e766e17](https://github.com/AI-Substrate/umx/commit/e766e175405304c33cad02af7ac13ee1f2156295))
* add graph hubs (umx hub set|list|show|fill|remove) ([cb826aa](https://github.com/AI-Substrate/umx/commit/cb826aa51ea16e789a904adb27d4d99809372268))
* add password-protected LAN graph view ([777ce68](https://github.com/AI-Substrate/umx/commit/777ce687b025187e3c2b49dd822613eb26d8c993))
* add soft declared values and std risk vocabulary ([16a1bd1](https://github.com/AI-Substrate/umx/commit/16a1bd134349527965d561ed7017cbb261086f09))
* check graph rules softly on writes and in check --rules ([667a636](https://github.com/AI-Substrate/umx/commit/667a636635bd799aed53a314344f7ce4525bd4c4))
* hash a cited source when the reference is added ([6ab311c](https://github.com/AI-Substrate/umx/commit/6ab311ce025c71e1e830ab1a53b80ca5163875b8))
* label search hits strong, fair or weak against the nodes searched (search-10) ([87c9c27](https://github.com/AI-Substrate/umx/commit/87c9c2771817196881e056b001d6c9d5030ad0d6))
* plain umx check warns about changed and missing cited files ([72433bd](https://github.com/AI-Substrate/umx/commit/72433bd352aa4e227fcda2d00a01414cb6699ed8))
* **ui:** add deterministic hub layouts and real node spacing ([ee5703a](https://github.com/AI-Substrate/umx/commit/ee5703a7bfd483b0fca122ef1fc7e7c06e494c52))
* **ui:** render declared hubs with band-constrained spine fans ([5185acf](https://github.com/AI-Substrate/umx/commit/5185acfa8544e37a762898dc0380998cf6f00ccc))
* umx query reads n.evidence_sources and r.evidence_sources ([0a7efb7](https://github.com/AI-Substrate/umx/commit/0a7efb77b74977d4ccda1f42e88482d56805d93b))


### Bug Fixes

* compare LAN password bytes directly ([48b7fb4](https://github.com/AI-Substrate/umx/commit/48b7fb4da806539912749640265bd02eae698e2c))
* keep placed std field kinds local to their types ([dcf2f9b](https://github.com/AI-Substrate/umx/commit/dcf2f9ba1a30176d3aea5be67423c5afa8c92a95))
* the daemon answers at once and reloads remembered graphs behind it ([324f1f8](https://github.com/AI-Substrate/umx/commit/324f1f8e58186cce6e34971393f561a1f6d2ae6a))
* **ui:** default to Normal and restore spine vertical spread ([53d14e5](https://github.com/AI-Substrate/umx/commit/53d14e52dbe36f921c5d88671707b4769fb3191d))
* **ui:** keep hub state aligned with full fan geometry ([8ea1224](https://github.com/AI-Substrate/umx/commit/8ea1224fa79586eb70a633ec7888b46f1db8a9ba))
* **ui:** make hub fans and controls visible ([3ee0636](https://github.com/AI-Substrate/umx/commit/3ee0636d622ea7179dfe0bdbcd91693e4586788d))
* **ui:** preserve positions on regrouping and avoid pan reprocessing ([a9a3e78](https://github.com/AI-Substrate/umx/commit/a9a3e78643e4aa2f93a834bd3d6d6687a5c57e34))
* **ui:** scale the spine layout to big graphs ([72afbd9](https://github.com/AI-Substrate/umx/commit/72afbd9db4ca17da3f0580094f019a6a106fa3ed))


### Documentation

* align hub layouts with declared hub model ([fdbe677](https://github.com/AI-Substrate/umx/commit/fdbe6776b19f038471bda29f9ba541e3069ce7a7))
* PRD 003, a node too long to check is reported and passed over ([3f26a1e](https://github.com/AI-Substrate/umx/commit/3f26a1e2aaf0dce7d9a2d92c831e00713f47ac51))
* PRD 003, declared field values are labels ([327a564](https://github.com/AI-Substrate/umx/commit/327a5640daff3d0439df5e92cef5bc98dbac80d6))
* PRD 003, graph rules (first draft) ([abafab5](https://github.com/AI-Substrate/umx/commit/abafab595011310246dd52ab9f78a87e3a478d0e))
* PRD 003, hubs are declared places where nodes gather ([7ba4963](https://github.com/AI-Substrate/umx/commit/7ba496301593e39c02879358263a94e174a4bfaa))
* PRD 003, large graphs continue rather than refuse ([1b8cf10](https://github.com/AI-Substrate/umx/commit/1b8cf101f960a4ef91d032c5cde8ef91c6d67500))
* PRD 003, meetings as hubs set up by the skill ([b15e0f9](https://github.com/AI-Substrate/umx/commit/b15e0f941c7d1364dc59fd1fc99e1113426c90ae))
* PRD 003, rule add stores undeclared vocabulary and warns ([49c5979](https://github.com/AI-Substrate/umx/commit/49c59796fadc93b85f961b32d928a33215ef01b0))
* PRD 003, rules nudge and never fence ([c172625](https://github.com/AI-Substrate/umx/commit/c172625ced3636fd1ebf1b7adce5f88bfed3c85a))
* PRD 003, when an edit joins a hub and how the view gets hubs ([2d7e626](https://github.com/AI-Substrate/umx/commit/2d7e62693d9e65ecf180545848de65d2f71ef081))
* **prd:** mark hub-07 shipped after protocol-108 live proof ([8acdb7f](https://github.com/AI-Substrate/umx/commit/8acdb7f14bc0ccba43c102a3fee1a7d17e2176a5))
* **ui:** define hub layout requirements ([171b5d7](https://github.com/AI-Substrate/umx/commit/171b5d79479c3b10dc02e0ae50837a0797dff75b))
* **ui:** record approved hub defaults and count ([1356d98](https://github.com/AI-Substrate/umx/commit/1356d986b88b9ceccbf468d301d7faa791156103))

## [0.5.2](https://github.com/AI-Substrate/umx/compare/v0.5.1...v0.5.2) (2026-11-24)


### Bug Fixes

* keep the graph view up while the daemon is busy ([b4a0bd2](https://github.com/AI-Substrate/umx/commit/b4a0bd2642ee3cf91e9190669599df2f1f44d0de))

## [0.5.1](https://github.com/AI-Substrate/umx/compare/v0.5.0...v0.5.1) (2026-11-24)


### Features

* **ui:** a node's panel shows its spines, and toggles them with the sidebar ([a6d4636](https://github.com/AI-Substrate/umx/commit/a6d4636291ecce1354111a58761ce31765c3f653))

## [0.5.0](https://github.com/AI-Substrate/umx/compare/v0.4.1...v0.5.0) (2026-11-23)


### ⚠ BREAKING CHANGES

* bin/serve.mjs takes --stdin instead of --token-stdin and reads no token line; the UiHost port drops UiLaunch.token and renames token() to code(); `umx ui --no-open` prints the plain URL and now works with JSON output. Any process or user on this machine that can reach 127.0.0.1 can read the view: the trust boundary is the user account (umx docs get ui).

### Features

* open the graph view with a plain local URL, no one-time token ([81a0c43](https://github.com/AI-Substrate/umx/commit/81a0c4388f3158863a202a613d624fb37e149b38))
* **ui:** spread a spine out, and stop a band's nodes piling up ([76aad7a](https://github.com/AI-Substrate/umx/commit/76aad7aeccdf28534dce62ff51925972cf4bb358))

## [0.4.1](https://github.com/AI-Substrate/umx/compare/v0.4.0...v0.4.1) (2026-11-23)


### Bug Fixes

* read the policy record once per request, not once per edge ([01ea6c4](https://github.com/AI-Substrate/umx/commit/01ea6c41d8fe0831564e98907e642bff2cc9301f))


### Performance

* **ui:** open graphs in a fraction of the time ([4b67a69](https://github.com/AI-Substrate/umx/commit/4b67a69d05d75391337243e07c4836eb14885cba))


### Documentation

* lead the README with entities across graphs, spines and evidence ([26c6fda](https://github.com/AI-Substrate/umx/commit/26c6fdad5d99d0f8a7d0a7e08813f36c8c3b4088))

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
