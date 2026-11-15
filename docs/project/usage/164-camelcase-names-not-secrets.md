---
id: 164
title: Long camelCase and PascalCase names, such as column names, are not flagged as secrets
route: ingest
status: current
source: ws002 F146 (plan 149), reported by a tester; rule from the security reviewer 2026-11-12
reqs: [req-49]
supersedes: none
---
## Do
Write column, field and class names as they are, for example a note that says
`totalOutstandingInvoiceAmountExcludingGST2024 is filled nightly`. A run with no `-` or `_` is not flagged as a
random token (`secret_suspected`, rule high_entropy_token) when it splits at each lowercase-to-capital and
letter-to-digit change into 3 or more words, where each word is a capital and lowercase letters (`Account`,
`balance`), an acronym of 2 to 5 capitals (`GST`, `ID`, at most 2 of them) or up to 4 digits (`2024`), and the words
average 4 or more characters.

## Don't
Don't expect every camelCase-looking run to pass. An acronym straight before a capitalised word (`UTCFor`), random
capitals (`HIstory`), an acronym of 6 capitals, 3 acronyms or a run of 5 digits keep it a candidate. If such a text
warns and is safe to send, allow that exact text with `umx embeddings allow --node <node> --field <field>` (usage
091). There is no per-graph or per-field allow.

## Example
```bash
umx node add --type problem --title "Invoice totals drift" \
  --note "totalOutstandingInvoiceAmountExcludingGST2024 is filled by the nightly job"
# no secret_suspected warning; the note is embedded as usual
```

## Why
A long column name has a digit, both cases and enough variety to pass the entropy rule, so it was held from hosted
embedders like a token. Security's word rule lets real names through and, measured on 10,000 uniformly random
base64url tokens at 32, 40 and 64 bytes, exempts none of those the entropy rule catches.
