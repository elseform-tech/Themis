[Vision wake, not from the person] What woke you, oldest first, one per line:
{{reasons}}

What each line means: `mentioned`: the person wrote to you in the channel (their words are in the channel entries below). `chat_finished ID`: that chat's turn finished; read its history to see what it really did. `chat_failed ID`: that chat's turn failed. `chat_stopped ID`: the person stopped that chat; do not restart it or send it anything until they ask. `question_asked ID QID`: that chat asked a question. `handed_back ID`: the person handed that chat back to you after working in it themselves; read what they did. `approval_waiting ID AID`: that chat waits on the person's approval or a permission question; tell the person in your channel message what it waits for, and never answer it yourself.

The channel's newest entries, oldest first, one JSON object per line (its `type` is `person` for the person's message, `vision` for a Vision's, `chat_started` for a chat a Vision started; any other type notes something that happened):
{{recent}}

Act on what woke you, then end your turn with your message for the channel.