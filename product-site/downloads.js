// releases.json separates the product described here from available installation packages.
(() => {
  const links = [...document.querySelectorAll('[data-download]')];
  const status = document.getElementById('download-status');
  const retry = document.getElementById('download-retry');
  if (!links.length || !status || !retry) return;
  let loading = false;
  const disable = link => {
    link.removeAttribute('href');
    link.setAttribute('aria-disabled', 'true');
    link.tabIndex = -1;
    link.title = '此平台下载暂不可用';
  };
  async function load() {
    if (loading) return;
    loading = true;
    retry.hidden = true;
    status.hidden = false;
    status.textContent = '正在加载下载信息…';
    links.forEach(disable);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 8000);
    try {
      const response = await fetch(new URL('./releases.json', document.baseURI), {
        cache: 'no-store', signal: controller.signal,
      });
      if (!response.ok) throw new Error('release unavailable');
      const release = await response.json();
      if (!release || typeof release.version !== 'string' || !/^\d+\.\d+\.\d+$/.test(release.version)
          || !release.downloads || typeof release.downloads !== 'object' || Array.isArray(release.downloads)) {
        throw new Error('invalid release');
      }
      const validVersion = value => typeof value === 'string' && /^\d+\.\d+\.\d+$/.test(value);
      const productVersion = release.product_version ?? release.version;
      if (!validVersion(productVersion) || (release.versions != null && (typeof release.versions !== 'object' || Array.isArray(release.versions)))) throw new Error('invalid versions');
      const packageVersions = links.map(link => release.versions?.[link.dataset.download] ?? release.version);
      if (!packageVersions.every(validVersion)) throw new Error('invalid package version');
      // Validate all URLs and versions before changing any link or version label.
      const urls = links.map(link => {
        const raw = release.downloads[link.dataset.download];
        if (raw == null || raw === '') return null;
        if (typeof raw !== 'string') throw new Error('invalid URL');
        const url = new URL(raw);
        if (url.protocol !== 'https:' || !url.hostname || url.username || url.password) throw new Error('invalid URL');
        return url.href;
      });
      if (!urls.some(Boolean)) throw new Error('no downloads');
      document.querySelectorAll('[data-product-version]').forEach(label => {
        label.textContent = (label.dataset.prefix || 'v') + productVersion + (label.dataset.suffix || '');
      });
      document.querySelectorAll('[data-release-version]').forEach(label => {
        const version = release.versions?.[label.dataset.releasePlatform] || release.version;
        label.textContent = (label.dataset.prefix || 'v') + version;
      });
      links.forEach((link, i) => {
        if (!urls[i]) return;
        link.href = urls[i];
        link.removeAttribute('aria-disabled');
        link.removeAttribute('tabindex');
        link.removeAttribute('title');
      });
      status.textContent = urls.every(Boolean) ? '' : '部分平台暂未提供下载。';
      status.hidden = urls.every(Boolean);
    } catch {
      status.textContent = '下载信息暂时无法加载，请稍后重试。';
      retry.hidden = false;
    } finally {
      clearTimeout(timeout);
      loading = false;
    }
  }
  retry.addEventListener('click', () => void load());
  void load();
})();
