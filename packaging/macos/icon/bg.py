import json, os, sys
here = os.path.dirname(os.path.abspath(sys.argv[0]))
m = json.load(open(os.path.join(here, 'mark.json')))
# the aperture as a cover on the left panel: scaled k, its centre at (cx, cy)
k, cx, cy = 0.66, 96.0, 236.0
T = lambda p: (cx + (p[0]-512)*k, cy + (p[1]-512)*k)
pl = lambda pts: " ".join(f"{x:.1f},{y:.1f}" for x, y in map(T, pts))
edges = "\n".join(f'    <polyline points="{pl(e)}"/>' for e in m['edges'])
R = m['disc']['R']*k
spec = dict(plate=m['plate']['color'], paper=m['paper'], c1=m['grad']['c1'], c2=m['grad']['c2'], k=k, cx=cx, cy=cy, R=R, gap=m['gap']*k, edges=[[T(p) for p in e] for e in m['edges']], opening=[T(p) for p in m['opening']])
json.dump(spec, open(os.path.join(here, 'bg.json'), 'w'))
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="660" height="400" viewBox="0 0 660 400">
<!-- Jilo Studio DMG window: a dark panel with the app icon's aperture as a cover on the left, paper under
     both icons (Finder draws their labels in black), an editing-track arrow from the app to
     Applications. No text, so rendering needs no fonts. -->
<defs>
  <clipPath id="panel"><rect width="232" height="400"/></clipPath>
  <clipPath id="disc"><circle cx="{cx:.1f}" cy="{cy:.1f}" r="{R:.1f}"/></clipPath>
  <linearGradient id="blades" gradientUnits="userSpaceOnUse" x1="{cx-R:.1f}" y1="{cy-R:.1f}" x2="{cx+R:.1f}" y2="{cy+R:.1f}">
    <stop offset="0" stop-color="{m['grad']['c1']}"/><stop offset="1" stop-color="{m['grad']['c2']}"/>
  </linearGradient>
</defs>

<rect width="660" height="400" fill="{m['paper']}"/>
<g clip-path="url(#panel)">
  <rect width="232" height="400" fill="{m['plate']['color']}"/>
  <!-- the Jilo Studio icon's aperture (assets/brand/jilo/jilo-studio.svg), its gaps and opening in the panel colour -->
  <g clip-path="url(#disc)">
    <circle cx="{cx:.1f}" cy="{cy:.1f}" r="{R:.1f}" fill="url(#blades)"/>
    <g fill="none" stroke="{m['plate']['color']}" stroke-width="{m['gap']*k:.1f}" stroke-linecap="round" stroke-linejoin="round">
{edges}
    </g>
    <polygon points="{pl(m['opening'])}" fill="{m['plate']['color']}"/>
  </g>
</g>
<line x1="232.5" y1="0" x2="232.5" y2="400" stroke="#15131D" stroke-width="1.2"/>
<line x1="236.5" y1="0" x2="236.5" y2="400" stroke="#15131D" stroke-width="0.5"/>

<line x1="267" y1="88.5" x2="624" y2="88.5" stroke="#15131D" stroke-width="0.6"/>
<rect x="267" y="84" width="22" height="9" rx="3" fill="{m['grad']['c2']}" stroke="#15131D" stroke-width="0.8"/>

<rect x="404" y="199" width="22" height="12" rx="3.5" fill="#15131D"/>
<rect x="430" y="199" width="12" height="12" rx="3.5" fill="#15131D" fill-opacity="0.55"/>
<rect x="446" y="199" width="12" height="12" rx="3.5" fill="#15131D" fill-opacity="0.25"/>
<rect x="427.2" y="194" width="1.6" height="22" rx="0.8" fill="#15131D"/>
<path d="M466,192 L490,205 L466,218 Z" fill="{m['grad']['c2']}" stroke="#15131D" stroke-width="1.2" stroke-linejoin="round"/>
<line x1="267" y1="335.5" x2="624" y2="335.5" stroke="#15131D" stroke-width="0.6"/>
</svg>
'''
open(os.path.join(here, 'background.svg'), 'w').write(svg)
