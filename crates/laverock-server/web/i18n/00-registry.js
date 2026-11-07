// ---------------------------------------------------------------- i18n
//
// Every `web/i18n/*.js` file registers its strings through `addI18n`; the
// build concatenates those files before the runtime below that defines `t()`.

const I18N = { zh: {}, en: {} };
const addI18n = (fragments) => {
  for (const [code, entries] of Object.entries(fragments)) {
    Object.assign(I18N[code] ??= {}, entries);
  }
};
