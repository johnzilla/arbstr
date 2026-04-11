---
phase: quick
plan: 260411-afb
type: execute
wave: 1
depends_on: []
files_modified:
  - docs/index.html
autonomous: true
---

<objective>
Fix the "full documentation on GitHub" link on arbstr.com landing page.
Currently points to arbstr-node repo instead of arbstr repo.
</objective>

<tasks>
<task type="auto">
  <name>Task 1: Fix GitHub docs link</name>
  <files>docs/index.html</files>
  <action>Change href from arbstr-node#readme to arbstr#readme on line 95</action>
  <verify>grep -q 'johnzilla/arbstr#readme' docs/index.html && echo "PASS" || echo "FAIL"</verify>
  <done>Link corrected to point to arbstr repo</done>
</task>
</tasks>
