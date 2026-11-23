import math, cairosvg
from shapely.geometry import Polygon, LineString, box
from shapely.ops import unary_union
from PIL import Image

def se(cx,cy,rx,ry_top,ry_bot,n_top,n_bot,steps=720):
    pts=[]
    for i in range(steps):
        t=2*math.pi*i/steps; c,s=math.cos(t),math.sin(t)
        n = n_bot if s>0 else n_top; ry = ry_bot if s>0 else ry_top
        pts.append((cx+rx*math.copysign(abs(c)**(2/n),c), cy+ry*math.copysign(abs(s)**(2/n),s)))
    return Polygon(pts)

# pronotum (head shield): small, broad dome
pron = se(512,420,150,96,40,2.4,4)
# elytra: square shoulders, long tapered rear
ely  = se(512,500,226,70,330,4,1.9)
shape = unary_union([pron, ely])

def arc(y0,sag,w,x0=250,x1=774):
    xs=[x0+i*(x1-x0)/100 for i in range(101)]; hw=(x1-x0)/2
    return LineString([(x,y0+sag*(1-((x-512)/hw)**2)) for x in xs]).buffer(w/2,cap_style=2)
# split: thin at the shoulders, opening toward the tail
split = Polygon([(512-12,470),(512+12,470),(512+30,850),(512-30,850)])
gaps = unary_union([arc(438,34,30), arc(620,40,30), split])
body = shape.difference(gaps).buffer(-10,join_style=1).buffer(10,join_style=1,resolution=16)

def path(g):
    out=[]
    for p in (g.geoms if hasattr(g,'geoms') else [g]):
        for r in [p.exterior,*p.interiors]:
            out.append("M"+" L".join(f"{x:.1f} {y:.1f}" for x,y in r.coords)+" Z")
    return " ".join(out)

top = pron.bounds[1]
svg=f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="g" x1="330" y1="200" x2="700" y2="840" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#4C8DFF"/><stop offset="1" stop-color="#3B4FD8"/>
    </linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="#111214"/>
  <path fill="url(#g)" d="{path(body)}"/>
  <g stroke="url(#g)" stroke-width="30" stroke-linecap="round" fill="none">
    <path d="M462 {top+6} Q438 {top-60} 370 {top-104}"/>
    <path d="M562 {top+6} Q586 {top-60} 654 {top-104}"/>
  </g>
</svg>'''
open('databug-icon.svg','w').write(svg)
for s in [1024,512,256,128,64,32,16]:
    cairosvg.svg2png(bytestring=svg.encode(),write_to=f'databug-{s}.png',output_width=s,output_height=s)
sizes=[256,128,64,32,16]; W=sum(sizes)+40*6; H=336
sheet=Image.new('RGBA',(W,H),(236,236,238,255)); x=40
for s in sizes:
    im=Image.open(f'databug-{s}.png'); sheet.paste(im,(x,(H-s)//2),im); x+=s+40
sheet.save('size-check.png')
print(body.bounds, top)
