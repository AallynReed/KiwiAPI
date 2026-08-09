/* Bridges the ES-module DDS decoder onto `window` for the classic (non-module)
   page scripts, then announces it. Shared by /updates and /store, which both
   render .dds textures. Was an inline `<script type="module">` in each
   template until the CSP dropped 'unsafe-inline'. */
import { decodeDDS } from '/static/pkfx/dds.js';

window.decodeDDS = decodeDDS;
document.dispatchEvent(new Event('btt-dds-ready'));
