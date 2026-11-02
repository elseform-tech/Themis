---
name: todo
description: Keep a to-do list with todo_write for any task of three or more steps.
---

# To-do list

For a task with three or more steps, call `todo_write` before you start: one item per result someone could check, in order. Keep exactly one item `in_progress`. When an item is done and shown working, mark it `completed` with a one-line `outcome` in the same call that starts the next. Pass each item's `id` to keep it. Add items you discover; remove ones that no longer apply.

Example, asked to "add a CSV export and fix the date filter":

```json
{"todos":[
 {"title":"CSV export downloads every row","status":"in_progress"},
 {"title":"Date filter keeps the end date","status":"pending"},
 {"title":"Verify both in the running app","status":"pending"}]}
```

After the export works:

```json
{"todos":[
 {"id":"t1","title":"CSV export downloads every row","status":"completed","outcome":"Downloaded 120 rows, opened in Numbers"},
 {"id":"t2","title":"Date filter keeps the end date","status":"in_progress"},
 {"id":"t3","title":"Verify both in the running app","status":"pending"}]}
```

Never mark an item completed you have not checked. Items the person asked for ("you asked") stay until they are done.
