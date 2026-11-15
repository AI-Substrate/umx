---
id: 009
title: Edge direction is meaning; read the echoed sentence
route: build
status: current
source: req-59; live-concept-graph method
reqs: [req-59, req-13]
supersedes: none
---
## Do
`umx edge add <from> <relation> <to>` reads as a sentence and echoes it ("Approval delays blocks Launch date"). Check the sentence says what you meant before moving on.

## Don't
Don't add an edge when the relation overstates the claim ("undercuts" when it only "tests"); pick a gentler relation or add one to the vocabulary.

## Example
```bash
umx edge add <question> blocks <deliverable>
```

## Why
`blocks` and `conflicts_with` produce findings nobody stated (req-33), so direction must be right.
