// Background jobs: start a long request, then follow its replayable event
// stream. The server keeps the job and its artifacts, so reconnecting (a
// page refresh, a dropped SSE connection) resumes instead of losing work.
//
//   const result = await runJob('/api/engines/zipvoice/synthesize', form, {
//     snapshot: (s) => showProgress(s.progress),
//     stage: (e) => setStatus(e.message),
//     progress: (e) => setProgress(e.value),
//     audio: (e) => addTake(e),
//     artifact: (e) => addDownload(e),
//   });
//
// `runJob` resolves with the terminal snapshot (or the `done` payload) and
// rejects on a failed job.

async function runJob(url, formData, handlers = {}) {
  const response = await fetch(url, {
    method: 'POST',
    body: formData,
    headers: {
      'x-laverock-job': '1',
      'Accept-Language': document.documentElement.lang || '',
    },
  });
  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try {
      const body = await response.json();
      if (body.error) message = body.error;
    } catch (_) {
      // Not JSON; keep the status text.
    }
    throw new Error(message);
  }
  const { job_id: id } = await response.json();
  return watchJob(id, handlers);
}

// Follow a job's events. Resolves when the job reaches a terminal state;
// EventSource reconnects by itself, and the server replays what was missed.
function watchJob(id, handlers = {}) {
  return new Promise((resolve, reject) => {
    const source = new EventSource(`/api/jobs/${id}/events`);
    let settled = false;

    const finish = (value) => {
      if (settled) return;
      settled = true;
      source.close();
      resolve(value);
    };
    const fail = (error) => {
      if (settled) return;
      settled = true;
      source.close();
      reject(error);
    };
    const dispatch = (name, data) => {
      const handler = handlers[name];
      if (handler) handler(data);
    };
    const parse = (event) => {
      try {
        return JSON.parse(event.data);
      } catch (_) {
        return {};
      }
    };

    source.addEventListener('snapshot', (event) => {
      const snapshot = parse(event);
      dispatch('snapshot', snapshot);
      if (snapshot.status === 'done') finish(snapshot);
      else if (snapshot.status === 'error') fail(new Error(snapshot.message || 'job failed'));
      else if (snapshot.status === 'cancelled') finish({ ...snapshot, cancelled: true });
    });
    source.addEventListener('stage', (event) => dispatch('stage', parse(event)));
    source.addEventListener('progress', (event) => dispatch('progress', parse(event)));
    source.addEventListener('audio', (event) => dispatch('audio', parse(event)));
    source.addEventListener('artifact', (event) => dispatch('artifact', parse(event)));
    source.addEventListener('done', (event) => {
      const done = parse(event);
      finish(done.cancelled ? { cancelled: true, ...done } : done);
    });
    // A job error carries data; a transport error does not, and EventSource
    // retries those on its own (the reconnect replays missed events).
    source.addEventListener('error', (event) => {
      if (!event.data) return;
      fail(new Error(parse(event).message || 'job failed'));
    });
  });
}

async function cancelJob(id) {
  const response = await fetch(`/api/jobs/${id}/cancel`, { method: 'POST' });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
}

async function jobSnapshot(id) {
  const response = await fetch(`/api/jobs/${id}`);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}
