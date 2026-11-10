import { build } from 'esbuild';

// An output path lets esbuild extract CSS imported by real components even
// though fixtures serve the generated files from memory.
export function buildBrowserFixture(options) {
  return build({ outfile: '/tmp/edenvoice-browser-fixture/app.js', ...options });
}

export function fixtureStyles(bundle) {
  return bundle.outputFiles.filter(file => file.path.endsWith('.css')).map(file => file.text).join('\n');
}
