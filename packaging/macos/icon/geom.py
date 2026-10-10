import json, math, sys, os
# Jilo Studio: an aperture of six curved blades on Apple's 1024 macOS icon grid (824 plate, 100 margin).
# usage: geom.py [rho=420 r=70 gap=22 ...] [plate=#rrggbb c1=#rrggbb c2=#rrggbb paper=#rrggbb]
C = 512.0
P = dict(R=262.0, r=74.0, rho=900.0, bend=1.0, gap=22.0, rot=-12.0, overshoot=40.0)
# plate and blade gradient (top left to bottom right)
COL = dict(plate='#15131D', c1='#E3C8FF', c2='#7B2FF7', paper='#F2EFF7')
for a in sys.argv[1:]:
    k, v = a.split('=')
    if k in COL: COL[k] = v
    else: P[k] = float(v)
here = os.path.dirname(os.path.abspath(sys.argv[0]))

def arc(i):
    """Blade edge i: an arc of radius rho tangent to the inner circle (radius r)."""
    a = math.radians(P['rot'] + i*60)
    u = (math.cos(a), math.sin(a)); d = (-u[1], u[0])
    T = (C + P['r']*u[0], C + P['r']*u[1])
    n = (-u[0]*P['bend'], -u[1]*P['bend'])
    rho = P['rho']
    Q = (T[0] + rho*n[0], T[1] + rho*n[1])
    def at(s):
        ph = s/rho
        return (Q[0] + rho*(-n[0]*math.cos(ph) + d[0]*math.sin(ph)), Q[1] + rho*(-n[1]*math.cos(ph) + d[1]*math.sin(ph)))
    return at

L = 900.0; N = 3600
arcs = [arc(i) for i in range(6)]
samples = [[(s, f(s)) for s in [-L + 2*L*k/N for k in range(N+1)]] for f in arcs]

def meet(i, j):
    """Parameter on arc i where it crosses arc j (the crossing nearest the centre)."""
    best = None
    sj = samples[j]
    for s, p in samples[i]:
        if math.hypot(p[0]-C, p[1]-C) > P['R']: continue
        dmin = min(math.hypot(p[0]-q[0], p[1]-q[1]) for _, q in sj[::4])
        if best is None or dmin < best[0]: best = (dmin, s)
    return best[1]

# vertex V_i: arc i crosses arc i-1; the opening's side i runs from V_i to V_{i+1}
start = [meet(i, (i-1) % 6) for i in range(6)]
end = [meet(i, (i+1) % 6) for i in range(6)]
edges, opening = [], []
for i in range(6):
    f = arcs[i]; s0, s1 = start[i], end[i]
    step = 1.0 if s1 >= s0 else -1.0
    pts, s = [], s0
    while True:
        p = f(s); pts.append(p)
        if math.hypot(p[0]-C, p[1]-C) > P['R'] + P['overshoot']: break
        s += step*2
        if abs(s) > L: break
    edges.append(pts)
    side, s = [], s0
    while (s - s1)*step <= 0:
        side.append(f(s)); s += step*2
    opening += side
spec = dict(size=1024, plate=dict(x=100, y=100, w=824, h=824, r=185, color=COL["plate"]), paper=COL["paper"],
            disc=dict(cx=C, cy=C, R=P['R']), gap=P['gap'], edges=edges, opening=opening,
            grad=dict(x1=C-P['R'], y1=C-P['R'], x2=C+P['R'], y2=C+P['R'], c1=COL["c1"], c2=COL["c2"]))
json.dump(spec, open(os.path.join(here, 'mark.json'), 'w'))
pl = lambda pts: " ".join(f"{p[0]:.1f},{p[1]:.1f}" for p in pts)
lines = "\n".join(f'      <polyline points="{pl(e)}"/>' for e in edges)
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="blades" gradientUnits="userSpaceOnUse" x1="{C-P['R']:.0f}" y1="{C-P['R']:.0f}" x2="{C+P['R']:.0f}" y2="{C+P['R']:.0f}">
      <stop offset="0" stop-color="{COL['c1']}"/>
      <stop offset="1" stop-color="{COL['c2']}"/>
    </linearGradient>
    <clipPath id="disc"><circle cx="{C:.0f}" cy="{C:.0f}" r="{P['R']:.0f}"/></clipPath>
  </defs>
  <!-- Apple's macOS icon grid: an 824 x 824 plate with 100 px margins. -->
  <rect x="100" y="100" width="824" height="824" rx="185" fill="{COL['plate']}"/>
  <!-- Jilo Studio: an aperture of six curved blades in a violet gradient, its gaps and opening cut to the plate. -->
  <g clip-path="url(#disc)">
    <circle cx="{C:.0f}" cy="{C:.0f}" r="{P['R']:.0f}" fill="url(#blades)"/>
    <g fill="none" stroke="{COL['plate']}" stroke-width="{P['gap']:.0f}" stroke-linecap="round" stroke-linejoin="round">
{lines}
    </g>
    <polygon points="{pl(opening)}" fill="{COL['plate']}"/>
  </g>
</svg>
'''
open(os.path.join(here, 'jilo-studio.svg'), 'w').write(svg)
print("ok", [round(s) for s in start], [round(s) for s in end])
