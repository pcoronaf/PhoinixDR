// Resolves the download buttons to the assets of the latest GitHub
// release. Asset names carry the version, so the static links point at the
// release page and are upgraded to direct downloads when the API answers.
(function () {
  var api = "https://api.github.com/repos/pcoronaf/PhoinixDR/releases/latest";
  var links = document.querySelectorAll("a[data-asset]");
  var versions = document.querySelectorAll("[data-latest-version]");
  if (!links.length && !versions.length) return;
  function escapeRegExp(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }
  function toPattern(glob) {
    return new RegExp("^" + glob.split("*").map(escapeRegExp).join(".+") + "$");
  }
  fetch(api, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(new Error(String(r.status))); })
    .then(function (release) {
      var assets = release.assets || [];
      links.forEach(function (a) {
        var pattern = toPattern(a.getAttribute("data-asset"));
        var hit = null;
        assets.forEach(function (x) { if (!hit && pattern.test(x.name)) hit = x; });
        if (!hit) return;
        a.href = hit.browser_download_url;
        if (a.hasAttribute("data-asset-name")) a.textContent = hit.name;
      });
      versions.forEach(function (el) { el.textContent = release.tag_name; });
    })
    .catch(function () { /* keep the static links to the release page */ });
})();
