---
id: 157
title: node find checks a short query word by word
route: explore
status: current
source: ws004 s30 (plan 132), user report 2026-11-12; design agreed with the embeddings-expert and the graph-storage-expert
reqs: [req-57, req-135]
supersedes: none
---
## Do

Give `umx node find` the words of the title you want, typos allowed. For a query of one to three words, a title
is returned when it holds all your words (usage 116), when the whole title is a close match (score 0.8 or more),
or when each of your words is within a typo of one of the title's words (about one edit per four letters, at
least one). A query word that joins title words ("approvalworkflow") also counts. `umx node find --provenance` checks provenance titles the same way.

## Don't

Don't expect a short name to find a different name that only shares letters or one word: "Jacqueline Moreno" no
longer finds "Jacqueline Salazar", and two-word names no longer find unrelated two-word names at 0.4 to 0.6.
Find a person by both names spelled closely, or by one name alone (a held word is always found).
The same holds for one word: `control` no longer finds a two-word title with no word close to it, such as
"Central Trolley" at 0.5. It still finds "Controller Group", whose word holds it.

Don't expect numbers to be fuzzy: `2025` does not match `2026` word by word. A short title that is a close match
as a whole can still be returned, ranked below the exact one.

## Example

    umx node find "Rebceca Hamilotn"
    # finds "Rebecca Hamilton" (each word one swap away; whole-title score 0.706)
    umx node find "Jacqueline Moreno"
    # no longer returns "Jacqueline Salazar" (0.595 on whole-title letters alone)

## Why

Field use found two-word person names returning unrelated names at 0.4 and 0.545. On 2,000 synthetic names and 300
names none of them held, whole-title similarity alone returned about 30 names per query; checking each word
returns a few close spellings at most, and every typo of a held name is still found.
