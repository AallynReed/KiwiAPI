import { readFileSync } from 'node:fs';
import { parsePkfx } from '../site/static/pkfx/parser.js';
import { buildEffect } from '../site/static/pkfx/model.js';
import { System } from '../site/static/pkfx/sim.js';
let seed=12345; const rng=()=>{seed=(seed*1103515245+12345)&0x7fffffff;return seed/0x7fffffff;};
const file = process.argv[2];
const doc = parsePkfx(readFileSync(file,'utf8'));
const eff = buildEffect(doc, rng);
const roots = eff.layers.filter(l=>!l.isChild).length, kids = eff.layers.filter(l=>l.isChild).length;
const trails = eff.layers.reduce((a,l)=>a+l.evolvers.filter(e=>e.type==='spawner').length,0);
const evts = eff.layers.reduce((a,l)=>a+Object.keys(l.events||{}).length,0);
console.log(`layers: ${eff.layers.length} (root ${roots}, child ${kids}) · trail-emitters ${trails} · event-decls ${evts} · attrs ${Object.keys(eff.attributes).length}`);
const sys = new System(eff, rng);
let peak=0, childAlivePeak=0;
for(let f=0;f<240;f++){ sys.update(1/60); let a=0,ca=0; for(const l of sys.layers){a+=l.count; if(l.L.isChild)ca+=l.count;} peak=Math.max(peak,a); childAlivePeak=Math.max(childAlivePeak,ca); }
console.log(`peak alive ${peak} · peak child-layer alive ${childAlivePeak}`);
