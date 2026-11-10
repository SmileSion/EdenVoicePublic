// Shared vector master for the approved leaf / voice-ring identity.
// Run with: node desktop/scripts/generate-brand.mjs (after npm ci in desktop).
import { mkdirSync, writeFileSync, copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
const root = fileURLToPath(new URL('../..', import.meta.url));
const brand = join(root, 'desktop/brand');
mkdirSync(brand, { recursive: true });
const leaves = `<path d="M205 285C155 183 226 80 351 35C308 89 328 159 268 195C237 213 214 240 205 285Z"/>
<path d="M187 319C121 301 98 252 122 195C131 241 181 230 187 319Z"/>`;
const ring = `<path d="M318 187C433 199 487 326 424 416C376 487 270 493 207 448C177 437 151 437 123 449C152 426 180 423 210 430C162 383 163 327 190 266C178 335 195 394 246 431C304 474 386 447 414 387C450 310 406 218 318 187Z"/>`;
const bars = [ [252,320,42], [283,293,96], [314,268,148], [345,293,96], [376,320,42] ]
 .map(([x,y,h])=>`<rect x="${x}" y="${y}" width="16" height="${h}" rx="8"/>`).join('');
const veins = `<path d="M204 277C224 204 262 142 309 91C262 132 224 185 204 245Z"/>`;
function svg(mode, tile = false, tray = false) {
 const light = mode === 'light';
 const start = light ? '#10d5b0' : '#d1fff0', end = light ? '#008de6' : '#65bdff';
 const fill = tray ? (light ? '#101820' : '#ffffff') : 'url(#leaf)';
 return `<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
<defs><linearGradient id="leaf" x1=".22" y1="0" x2=".85" y2="1"><stop stop-color="${start}"/><stop offset="1" stop-color="${end}"/></linearGradient><linearGradient id="tile" x2=".6" y2="1"><stop stop-color="${light?'#ffffff':'#203447'}"/><stop offset="1" stop-color="${light?'#e6f4fd':'#09131e'}"/></linearGradient></defs>
${tile?'<rect x="16" y="16" width="480" height="480" rx="112" fill="url(#tile)"/><rect x="17" y="17" width="478" height="478" rx="111" fill="none" stroke="'+(light?'#cbdce7':'#54748d')+'" stroke-opacity=".35" stroke-width="2"/>':''}
<g transform="${tile?'translate(45 50) scale(.79)':'translate(-12 0)'}" fill="${fill}">${leaves}${ring}${bars}${tray?'':`<g fill="${light?'#eafffa':'#143646'}" opacity=".85">${veins}</g>`}</g></svg>\n`;
}
for (const mode of ['light','dark']) {
 writeFileSync(join(brand,`logo-${mode}.svg`),svg(mode));
 const mark=svg(mode).replace('width="512" height="512"','x="0" y="0" width="120" height="120"');
 writeFileSync(join(brand,`wordmark-${mode}.svg`),`<svg xmlns="http://www.w3.org/2000/svg" width="560" height="120" viewBox="0 0 560 120">${mark}<text x="139" y="81" font-family="Inter, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="64" font-weight="400" letter-spacing="-2" fill="${mode==='light'?'#122638':'#f2fbff'}">Eden<tspan fill="${mode==='light'?'#008fc9':'#8cdef4'}">Voice</tspan></text></svg>`);
 writeFileSync(join(brand,`app-icon-${mode}.svg`),svg(mode,true));
 writeFileSync(join(brand,`tray-${mode}.svg`),svg(mode,false,true));
}
copyFileSync(join(brand,'app-icon-dark.svg'),join(root,'desktop/app-icon.svg'));
// A media-query-aware SVG works for favicons and ordinary external <img> elements.
const adaptive=svg('light',true).replace('<defs>', '<style>@media(prefers-color-scheme:dark){#leaf stop:first-child{stop-color:#d1fff0}#leaf stop:last-child{stop-color:#65bdff}#tile stop:first-child{stop-color:#203447}#tile stop:last-child{stop-color:#09131e}}</style><defs>');
writeFileSync(join(root,'product-site/assets/app-icon.svg'),adaptive);
writeFileSync(join(root,'server/internal/webadmin/static/app-icon.svg'),adaptive);
const cli = join(root,'desktop/node_modules/@tauri-apps/cli/tauri.js');
const temp = mkdtempSync(join(tmpdir(),'edenvoice-icons-'));
const icons=join(root,'desktop/src-tauri/icons');
try {
 execFileSync(process.execPath,[cli,'icon',join(brand,'app-icon-dark.svg'),'-o',temp],{stdio:'pipe'});
 const names=['32x32.png','64x64.png','128x128.png','128x128@2x.png','icon.png','icon.ico','icon.icns','Square30x30Logo.png','Square44x44Logo.png','Square71x71Logo.png','Square89x89Logo.png','Square107x107Logo.png','Square142x142Logo.png','Square150x150Logo.png','Square284x284Logo.png','Square310x310Logo.png','StoreLogo.png'];
 for(const name of names)copyFileSync(join(temp,name),join(icons,name));
 for(const mode of ['light','dark']) {
  const out=join(temp,mode);
  execFileSync(process.execPath,[cli,'icon',join(brand,`tray-${mode}.svg`),'-o',out,'--png','32'],{stdio:'pipe'});
  copyFileSync(join(out,'32x32.png'),join(icons,`tray-${mode}.png`));
 }
} finally { rmSync(temp,{recursive:true,force:true}); }
console.log('Updated vector masters, website/admin logos, desktop and tray icons.');
