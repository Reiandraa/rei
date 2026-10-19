// Independent of the React bundle so a missing or failed module never leaves
// an empty window. No storage, account access, model calls, or error-body logs.
(() => {
  const fallback = document.getElementById('rei-boot-fallback');
  const retry = document.getElementById('rei-boot-retry');
  function unavailable() {
    if (!fallback?.isConnected) return;
    document.getElementById('rei-boot-title').textContent = 'REI is taking longer to open';
    document.getElementById('rei-boot-detail').textContent = 'The desktop interface has not started yet. Try reopening it; your saved workspace stays on this computer.';
    fallback.setAttribute('aria-busy', 'false');
    fallback.querySelector('.boot-line').hidden = true;
    retry.hidden = false;
  }
  retry?.addEventListener('click', () => window.location.reload());
  window.addEventListener('error', unavailable, {once: true});
  const timeout = window.setTimeout(unavailable, 12000);
  window.addEventListener('rei-interface-mounted', () => {
    window.clearTimeout(timeout);
    fallback?.remove();
  }, {once: true});
})();
