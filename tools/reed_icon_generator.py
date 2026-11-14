import math, random, sys
def cub(p0,p1,p2,p3,t):
    u=1-t
    return (u*u*u*p0[0]+3*u*u*t*p1[0]+3*u*t*t*p2[0]+t*t*t*p3[0], u*u*u*p0[1]+3*u*u*t*p1[1]+3*u*t*t*p2[1]+t*t*t*p3[1])
def dcub(p0,p1,p2,p3,t):
    u=1-t
    x=3*u*u*(p1[0]-p0[0])+6*u*t*(p2[0]-p1[0])+3*t*t*(p3[0]-p2[0])
    y=3*u*u*(p1[1]-p0[1])+6*u*t*(p2[1]-p1[1])+3*t*t*(p3[1]-p2[1])
    n=math.hypot(x,y) or 1
    return (x/n,y/n)
def ribbon(curve, wfun, n=36):
    L=[];R=[]
    for i in range(n+1):
        t=i/n; p=cub(*curve,t); d=dcub(*curve,t); w=wfun(t)/2
        nx,ny=-d[1],d[0]
        L.append((p[0]+nx*w,p[1]+ny*w)); R.append((p[0]-nx*w,p[1]-ny*w))
    pts=L+R[::-1]
    return "M"+" L".join("%.1f %.1f"%p for p in pts)+"Z"
def add(a,b,k=1): return (a[0]+b[0]*k,a[1]+b[1]*k)
def rot(v,deg):
    r=math.radians(deg); return (v[0]*math.cos(r)-v[1]*math.sin(r), v[0]*math.sin(r)+v[1]*math.cos(r))
WIND=(1,0); DOWN=(0,1)

def reed(x0,y0,h,lean,col,plume=1.0,seed=1,detail=True,wind=1.0,sw=1.0,lside=1):
    rnd=random.Random(seed); out=[]
    P0=(x0,y0); P3=(x0+lean*h,y0-h); P1=(x0,y0-h*0.45); P2=(x0+lean*h*0.3,y0-h*0.8)
    st=(P0,P1,P2,P3)
    w0=h*0.016*sw; w1=h*0.006*sw
    # leaves (behind stalk)
    for i,(t,side) in enumerate([(0.24,0.55),(0.44,1.0),(0.62,0.75)]):
        A=cub(*st,t); tg=dcub(*st,t)
        d=rot(tg, lside*side*(26+rnd.uniform(-4,4)))
        ln=h*(0.40-0.07*i)*rnd.uniform(0.9,1.1)
        D=add(add(add(A,d,ln*0.72),WIND,ln*0.34*wind),DOWN,ln*0.22)
        B=add(A,d,ln*0.42); C=add(add(A,d,ln*0.7),WIND,ln*0.14*wind)
        W=h*0.02*sw
        out.append('<path d="%s" fill="%s"/>'%(ribbon((A,B,C,D),lambda s:W*(math.sin(math.pi*min(1,s*1.05))**0.75)*(1-0.25*s)+0.6), col['leaf']))
    out.append('<path d="%s" fill="%s"/>'%(ribbon(st,lambda t:w0+(w1-w0)*t),col['stalk']))
    for t in (0.24,0.44,0.62):
        p=cub(*st,t); w=(w0+(w1-w0)*t)*0.8
        out.append('<ellipse cx="%.1f" cy="%.1f" rx="%.1f" ry="%.1f" fill="%s"/>'%(p[0],p[1],w,w*0.55,col['node']))
    # plume
    pl=h*0.36*plume; tg=dcub(*st,1)
    Q0=P3; Q1=add(P3,tg,pl*0.38)
    Q3=add(add(add(P3,tg,pl*0.74),WIND,pl*0.42*wind),DOWN,pl*0.10)
    Q2=add(add(P3,tg,pl*0.72),WIND,pl*0.14*wind)
    ra=(Q0,Q1,Q2,Q3)
    n=22 if detail else 7
    for k in range(n):
        s=0.03+0.94*k/(n-1); p=cub(*ra,s); t2=dcub(*ra,s); side=1 if k%2==0 else -1
        u=rot(t2, side*(34-14*s)+rnd.uniform(-4,4))
        L=pl*0.46*(0.38+0.62*math.sin(math.pi*min(1,0.12+s*0.95))**0.8)*(1-0.3*s)*rnd.uniform(0.88,1.08)
        E=add(add(add(p,u,L*0.78),WIND,L*0.34*wind),DOWN,L*0.26)
        B=add(p,u,L*0.42); C=add(add(p,u,L*0.72),WIND,L*0.1*wind)
        bw=pl*(0.05 if detail else 0.05)*sw
        out.append('<path d="%s" fill="%s" opacity="%.2f"/>'%(ribbon((p,B,C,E),lambda q:bw*(math.sin(math.pi*min(1,q*1.02+0.02))**0.6)*(1-0.3*q)+0.5,n=14),col['plume'],0.95))
        if detail:
            for da in (-9,9):
                u2=rot(u,da); E2=add(add(add(p,u2,L*0.7),WIND,L*0.38*wind),DOWN,L*0.3)
                out.append('<path d="M%.1f %.1f Q%.1f %.1f %.1f %.1f" fill="none" stroke="%s" stroke-width="%.1f" stroke-linecap="round" opacity=".55"/>'%(p[0],p[1],*add(p,u2,L*0.5),*E2,col['plume2'],max(1.2,pl*0.007)))
    out.append('<path d="%s" fill="%s"/>'%(ribbon(ra,lambda s:w1*1.25*(1-0.8*s)+0.8),col['stalk']))
    return "\n".join(out)

DARK=dict(stalk='#1b1b1b',leaf='#262626',node='#000',plume='#232323',plume2='#3d3d3d')
DARK2=dict(stalk='#2c2c2c',leaf='#383838',node='#111',plume='#343434',plume2='#555')
GOLD=dict(stalk='#C98A00',leaf='#E0A200',node='#9A6A00',plume='#F2B705',plume2='#FFD659')

def icon(variant='A'):
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">',
       '<rect x="64" y="64" width="896" height="896" rx="200" fill="#fff"/>',
       ]
    s.append('<g transform="translate(%d 0) translate(512 520) scale(1.1) translate(-512 -520)">'%(34 if variant=='A' else 76))
    s.append('<ellipse cx="470" cy="852" rx="270" ry="13" fill="#000" opacity=".06"/>')
    if variant=='A':
        s.append(reed(300,850,300,0.10,DARK2,0.95,3,wind=0.8))
        s.append(reed(640,850,280,0.12,DARK2,0.95,5,wind=0.8))
        s.append(reed(380,852,390,0.08,DARK,1.0,7,wind=0.8))
        s.append(reed(560,852,350,0.10,DARK,1.0,9,wind=0.8))
        s.append(reed(462,856,520,0.05,GOLD,1.12,11,wind=0.8))
    else:
        s.append(reed(250,850,320,0.34,DARK2,0.95,3,wind=1.25))
        s.append(reed(520,850,290,0.36,DARK2,0.95,5,wind=1.25))
        s.append(reed(320,852,400,0.30,DARK,1.0,7,wind=1.25))
        s.append(reed(450,852,360,0.33,DARK,1.0,9,wind=1.25))
        s.append(reed(385,856,530,0.24,GOLD,1.12,11,wind=1.25))
    s.append('</g></svg>'); return "\n".join(s)

def glyph(main='#F2B705', ink='#111', variant='A'):
    # 菜单栏用的简化版：三根，剪影，线条加粗保证小尺寸清楚
    c=dict(stalk=ink,leaf=ink,node=ink,plume=ink,plume2=ink); g=dict(stalk=main,leaf=main,node=main,plume=main,plume2=main)
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">']
    if variant=='A':
        s.append(reed(20,98,48,0.10,c,1.3,3,detail=False,wind=0.9,sw=5.4))
        s.append(reed(66,98,42,0.12,c,1.3,5,detail=False,wind=0.9,sw=5.4))
        s.append(reed(42,99,68,0.06,g,1.3,11,detail=False,wind=0.9,sw=4.4))
    else:
        s.append(reed(10,98,48,0.36,c,1.3,3,detail=False,wind=1.2,sw=5.4))
        s.append(reed(50,98,42,0.38,c,1.3,5,detail=False,wind=1.2,sw=5.4))
        s.append(reed(28,99,68,0.28,g,1.3,11,detail=False,wind=1.2,sw=4.4))
    s.append('</svg>'); return "\n".join(s)

if __name__=='__main__':
    open('icon_a.svg','w').write(icon('A')); open('icon_b.svg','w').write(icon('B'))
    for v in 'AB':
        open('glyph_l_%s.svg'%v,'w').write(glyph(variant=v))
        open('glyph_d_%s.svg'%v,'w').write(glyph('#FFC83D','#F2F2F2',variant=v))

# 每个形态：(名字, 整体平移, [(x, 高, 倾斜, 颜色, 穗大小, 随机种子, 风)...])，黄色那根写在最后
D,D2,G='D','D2','G'
VARIANTS=[
 ('微风',(-6,4),[(428,290,-0.20,D2,0.95,21,0.75),(524,300,0.52,D2,0.95,5,1.15),(444,395,-0.07,D,1.0,23,0.85),(502,380,0.36,D,1.0,9,1.1),(488,430,0.20,D,1.0,7,1.0),(466,500,0.05,G,1.12,11,0.6)]),
 ('风大一些',(-40,4),[(430,300,0.02,D2,0.95,21,1.0),(524,300,0.80,D2,0.95,5,1.4),(446,390,0.22,D,1.0,23,1.2),(504,360,0.62,D,1.0,9,1.4),(488,410,0.44,D,1.0,7,1.3),(462,560,0.06,G,1.12,11,0.7)]),
 ('三根',(-4,4),[(452,380,0.14,D,1.0,7,0.95),(500,330,0.44,D2,0.95,5,1.15),(470,560,0.04,G,1.12,11,0.6)]),
 ('密一些',(-14,4),[(420,250,-0.24,D2,0.9,31,0.7),(436,330,-0.12,D2,0.95,21,0.8),(532,270,0.62,D2,0.9,5,1.2),(448,400,-0.02,D,1.0,23,0.85),(516,330,0.46,D2,0.95,33,1.15),(504,385,0.34,D,1.0,9,1.1),(478,440,0.12,D,1.0,35,0.95),(492,425,0.24,D,1.0,7,1.0),(464,570,0.05,G,1.12,11,0.6)]),
 ('黄的在左边',(-50,4),[(520,290,0.60,D2,0.95,5,1.2),(470,400,0.16,D,1.0,23,0.95),(500,360,0.42,D,1.0,9,1.1),(486,420,0.28,D,1.0,7,1.0),(448,560,-0.01,G,1.12,11,0.55)]),
 ('黄的在右边',(8,4),[(420,300,-0.16,D2,0.95,21,0.75),(436,370,-0.04,D,1.0,23,0.85),(456,410,0.08,D,1.0,7,0.9),(474,360,0.20,D2,0.95,9,1.0),(500,570,0.05,G,1.12,11,0.6)]),
 ('高矮差大',(-10,4),[(430,230,-0.16,D2,0.95,21,0.8),(522,220,0.55,D2,0.95,5,1.15),(446,300,-0.04,D,1.0,23,0.9),(502,280,0.38,D,1.0,9,1.1),(486,320,0.20,D,1.0,7,1.0),(466,575,0.045,G,1.15,11,0.6)]),
 ('穗大而蓬',(-20,10),[(430,280,-0.18,D2,1.25,21,0.8),(522,290,0.52,D2,1.25,5,1.15),(446,370,-0.05,D,1.3,23,0.9),(500,360,0.36,D,1.3,9,1.1),(486,400,0.20,D,1.3,7,1.0),(466,520,0.05,G,1.45,11,0.65)]),
]
COL={'D':DARK,'D2':DARK2,'G':GOLD}
def icon_v(i):
    name,(tx,ty),rs=VARIANTS[i]
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">','<rect x="64" y="64" width="896" height="896" rx="200" fill="#fff"/>',
       '<g transform="translate(%d %d)">'%(tx,ty),'<ellipse cx="490" cy="852" rx="200" ry="12" fill="#000" opacity=".07"/>']
    for (x,h,lean,c,pl,seed,w) in rs:
        s.append(reed(x,850 if c!='G' else 856,h,lean,COL[c],pl,seed,wind=w,sw=1.2 if c=='G' else 1.0))
    s.append('</g></svg>'); return "\n".join(s)
def glyph_v(i,main='#F2B705',ink='#111'):
    name,(tx,ty),rs=VARIANTS[i]
    c=dict(stalk=ink,leaf=ink,node=ink,plume=ink,plume2=ink); g=dict(stalk=main,leaf=main,node=main,plume=main,plume2=main)
    darks=[r for r in rs if r[3]!='G']; gold=[r for r in rs if r[3]=='G'][0]
    lo=min(darks,key=lambda r:r[2]); hi=max(darks,key=lambda r:r[2])
    pick=[lo] if lo is hi else [lo,hi]
    k=0.128; cx=sum(r[0] for r in rs)/len(rs)+ (max(r[2] for r in rs)*120)
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">']
    for (x,h,lean,cc,pl,seed,w) in pick:
        s.append(reed(50+(x-cx)*k*2.2,98,min(h*k,54),lean,c,1.3,seed,detail=False,wind=w,sw=5.4))
    (x,h,lean,cc,pl,seed,w)=gold
    s.append(reed(50+(x-cx)*k*2.2,99,min(h*k,76),lean,g,1.3,seed,detail=False,wind=w,sw=4.4))
    s.append('</svg>'); return "\n".join(s)
if __name__=='__main__':
    for i in range(len(VARIANTS)):
        open('v%d.svg'%(i+1),'w').write(icon_v(i))
        open('g%d_l.svg'%(i+1),'w').write(glyph_v(i)); open('g%d_d.svg'%(i+1),'w').write(glyph_v(i,'#FFC83D','#F2F2F2'))
