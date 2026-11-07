// Streaming playback: decode base64 WAV chunks and schedule them back to
// back on one AudioContext, so the first sentence plays while the rest is
// still being generated.
//
//   const player = createStreamPlayer();
//   player.prime();                 // inside the click handler
//   await player.push(base64Wav);   // one per `audio` event
//   player.finish();                // closes the context after the drain
//   player.stop();                  // user pressed stop / request failed

function createStreamPlayer() {
  const Ctx = window.AudioContext || window.webkitAudioContext;
  let ctx = null;
  let nextTime = 0;
  let scheduled = 0;
  let closed = false;
  let finished = false;
  const sources = new Set();

  const close = () => {
    if (closed) return;
    closed = true;
    for (const source of sources) {
      try {
        source.stop();
      } catch (_) {
        // Already ended or never started.
      }
    }
    sources.clear();
    if (ctx) {
      const dying = ctx;
      ctx = null;
      try {
        dying.close();
      } catch (_) {
        // Already closed.
      }
    }
  };

  const ensureContext = () => {
    if (!ctx && !closed && Ctx) {
      ctx = new Ctx();
      // The context is created inside the generate click, so the page has
      // a user gesture; resume() covers a context that started suspended.
      if (ctx.state === 'suspended') ctx.resume().catch(() => {});
    }
    return ctx;
  };

  return {
    // Create the context while the click gesture is still active.
    prime() {
      ensureContext();
    },

    async push(base64) {
      const context = ensureContext();
      if (!context) return;
      const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const buffer = await context.decodeAudioData(bytes.buffer);
      if (closed || !ctx) return;
      const source = ctx.createBufferSource();
      source.buffer = buffer;
      source.connect(ctx.destination);
      // Small lead so scheduling never lands in the past.
      const at = Math.max(nextTime, ctx.currentTime + 0.05);
      source.start(at);
      nextTime = at + buffer.duration;
      scheduled += buffer.duration;
      sources.add(source);
      source.onended = () => {
        sources.delete(source);
        if (finished && sources.size === 0) close();
      };
    },

    // No more chunks are coming; close the context once playback drains.
    finish() {
      finished = true;
      if (sources.size === 0) close();
    },

    stop: close,

    // Seconds of audio received so far, for the progress label.
    get duration() {
      return scheduled;
    },

    get active() {
      return !closed && sources.size > 0;
    },
  };
}
