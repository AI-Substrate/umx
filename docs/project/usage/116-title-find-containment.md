---
id: 116
title: node find also returns long titles holding all your words
route: explore
status: current
source: ws004 s24 (plan 084), user report 2026-11-11, design agreed with the graph-storage-expert
reqs: [req-57]
supersedes: none
---
## Do

Give `umx node find` a few of the title's words, spelled as in the title. A title holding every word you give
(whole words, any order; case, hyphens and punctuation do not matter) is found however long it is. Such a title
scores at least 0.5 and below a close match; shorter ones rank first. An exact title is still 1.0 and first.

## Don't

Don't expect a typo to be forgiven against a long title: typos are judged against the whole title, so
`aproval month` can miss a 14-word title holding `approval` and `month`. Drop or fix the misspelled word.
Don't read a find score of 0.5 to 0.625 as proof that all your words occur: that range can come from
containment or from fuzzy similarity of the whole title.
Don't look for words in notes, or another form of a word (`advocates` for `advocate`), with find: it reads titles
only and does not stem. An empty find hints `find_no_match`; use `umx search` there.
`node add` and `umx batch` still hint `near_duplicate` only for titles close as a whole, never for holding words.

## Example

    umx node find "month-end approval"
    # finds "Approval delays at month end for the regional finance team and the shared services desk"
    # at 0.571; by whole-title similarity alone it scored 0.355, under the 0.4 cut-off

## Why

Before this, find scored only how similar the whole title was, so a two-word query could never reach the 0.4
cut-off against a title of more than about 12 words, however exactly its words appeared. Testers found long
titles they knew were there returned nothing.
