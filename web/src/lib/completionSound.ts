let context: AudioContext | null = null;
let sound: Promise<AudioBuffer | null> | null = null;

// Woodblock-double.wav by hollandm (CC0): https://freesound.org/people/hollandm/sounds/692817/

/** Call during Send's user gesture so the webview permits later playback. */
export function prepareCompletionSound(): void {
  if (!("__TAURI_INTERNALS__" in window) || !("AudioContext" in window)) return;
  context ??= new AudioContext();
  if (context.state === "suspended") void context.resume().catch(() => {});
  const audioContext = context;
  sound ??= fetch("/sounds/completion-woodblock-double.mp3")
    .then(response => {
      if (!response.ok) throw new Error("Completion sound unavailable");
      return response.arrayBuffer();
    })
    .then(data => audioContext.decodeAudioData(data))
    .catch(() => { sound = null; return null; });
}

export function playCompletionSound(): void {
  if (!context || !sound) return;
  const audioContext = context;
  void sound.then(buffer => {
    if (!buffer || audioContext.state !== "running") return;
    const source = audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(audioContext.destination);
    source.start();
  });
}
