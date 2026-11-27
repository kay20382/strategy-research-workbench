"""依 MC Excel 的圖表配置繪圖；輸入只能為本次引擎結果的衍生資料。"""
import json
from pathlib import Path
import sys
import math
import statistics
from PIL import Image, ImageDraw, ImageFont

p=Path(sys.argv[1]);data=json.loads(p.read_text(encoding='utf-8'))
out=p.parent/'mc-charts';out.mkdir(exist_ok=True)
FULL=data.get('full_mc_layout',data.get('full_nh14_layout',False))
font=ImageFont.truetype('C:/Windows/Fonts/mingliu.ttc',22 if FULL else 12)
small=ImageFont.truetype('C:/Windows/Fonts/mingliu.ttc',20 if FULL else 11)

def axis_ticks(low,high,intervals):
    raw=(high-low)/intervals
    magnitude=10**math.floor(math.log10(raw))
    step=next(n*magnitude for n in (1,2,2.5,5,10) if n*magnitude>=raw)
    digits=max(0,math.ceil(-math.log10(step)))
    if not math.isclose(step*10**digits,round(step*10**digits),abs_tol=1e-9):
        digits+=1
    return [(i*step,f'{i*step:,.{digits}f}') for i in range(math.ceil(low/step),math.floor(high/step)+1)]

class Graph:
    def __init__(self,values,count,ylabel,xlabel,*,xmax=None,percent=False):
        self.image=Image.new('RGB',(800,800),'white');self.draw=ImageDraw.Draw(self.image)
        self.left,self.right,self.top,self.bottom=(165,775,26,678) if FULL else (100,786,14,730)
        self.xmax=xmax
        if percent:self.left=115
        self.count=max(count,1);lo,hi=min(values or [0]),max(values or [0])
        if lo==hi:
            lo,hi=min(0,lo),max(.01 if percent else 1,hi)
        span=hi-lo if xmax is not None else max(hi-lo,1)
        self.lo=lo-span*.035;self.hi=hi+span*.035
        ticks=axis_ticks(self.lo,self.hi,9)
        for at,label in ticks:
            y=self.y(at);self.dash((self.left,y),(self.right,y));self.draw.text((self.left-9,y),label if xmax is not None else f'{at:,.0f}',font=font,fill='black',anchor='rm')
        self.draw.line([(self.left,self.top),(self.left,self.bottom),(self.right,self.bottom)],fill='black')
        self.draw.text((440,744 if FULL else 766),xlabel,font=font,fill='black',anchor='mm')
        label=Image.new('RGBA',(300,28),'white');ImageDraw.Draw(label).text((150,14),ylabel,font=font,fill='black',anchor='mm')
        self.image.paste(label.rotate(90,expand=True),(25,205))
    def x(self,index):return self.left+7+index/(self.xmax if self.xmax is not None else max(self.count-1,1))*(self.right-self.left-14)
    def y(self,value):return self.bottom-(value-self.lo)/(self.hi-self.lo)*(self.bottom-self.top)
    def dash(self,a,b):
        x0,y0=a;x1,y1=b;length=max(abs(x1-x0),abs(y1-y0))
        for at in range(0,int(length),7):
            u=at/length;v=min(at+4,length)/length
            self.draw.line([(x0+(x1-x0)*u,y0+(y1-y0)*u),(x0+(x1-x0)*v,y0+(y1-y0)*v)],fill='#707070')
    def ticks(self,items):
        for index,label in items:
            x=self.x(index);self.dash((x,self.top),(x,self.bottom));self.draw.text((x,710 if FULL else 747),label,font=small,fill='black',anchor='mm')
    def bar(self,index,value,color,hatch=False):
        width=min(52,max(4,((self.right-self.left)/self.count)*.67));x=self.x(index);y0,y1=sorted([self.y(0),self.y(value)])
        box=[round(x-width/2),round(y0),round(x+width/2),round(y1)]
        if hatch:
            self.draw.rectangle(box,outline=color);l,t,r,b=box
            for k in range(l-(b-t),r+1,7):
                start=max(0,l-k);end=min(b-t,r-k)
                if start<=end:self.draw.line([(k+start,b-start),(k+end,b-end)],fill=color)
        else:self.draw.rectangle(box,fill=color)
    def dot(self,index,value,color='black',size=3):
        x,y=self.x(index),self.y(value);self.draw.rectangle((x-size,y-size,x+size,y+size),fill=color)
    def legend(self,items):
        groups=[[]];total=0
        for item in items:
            width=self.draw.textlength(item[0],font=small)+30
            if total+width>750 and groups[-1]:groups.append([]);total=0
            groups[-1].append((item,width));total+=width
        for row,group in enumerate(groups):
            x=400-sum(width for _,width in group)/2;y=770+row*21 if len(groups)>1 else 784
            for (text,color,hatch,*marker),width in group:
                if hatch:self.draw.rectangle((x,y-4,x+6,y+4),outline=color);self.draw.line((x,y+4,x+6,y-4),fill=color)
                elif marker and marker[0]=='line':self.draw.line((x-3,y,x+8,y),fill=color)
                elif marker and marker[0] in ('up','down'):self.triangle(self.draw,x+2,y,color,marker[0]=='up',4)
                else:self.draw.rectangle((x,y-2,x+5,y+3),fill=color)
                self.draw.text((x+12,y),text,font=small,fill='black',anchor='lm');x+=width
    def save(self,name):
        target=out/(name+'.png');self.image.save(target);return str(target)

    @staticmethod
    def triangle(draw,x,y,color,up,size):
        direction=-1 if up else 1
        draw.polygon([(x,y+direction*size),(x-size,y-direction*size),(x+size,y-direction*size)],fill=color)

trades=data['trades']
open_trade=data.get('open_trade')
excursions=[*trades,*([open_trade] if open_trade else [])]
trade_max=max((t['id'] for t in excursions),default=1)
trade_marker=2 if len(trades)>1200 else 5
green,red='#228b22','#ff0404'

def trade_graph(values,ylabel):
    g=Graph(values,0,ylabel,'交易編號',xmax=trade_max)
    g.ticks([(x,f'{x:,.0f}') for x,_ in axis_ticks(0,trade_max,min(8,trade_max))])
    return g

def trade_dots(name,key,population,label,sign=0,show_open=False):
    values=[t[key] for t in population]
    if show_open and open_trade:values.append(open_trade[key])
    g=trade_graph(values,label+' (¤)')
    if population:
        y=g.y(statistics.mean(t[key] for t in population));g.draw.line([(g.left,y),(g.right,y)],fill='black')
    gain_color=green if sign else '#00ff00'
    legend=[(label if not sign else label+'交易','black',False)]
    if key=='net':
        if sign>=0:legend.append(('獲利的極端交易',gain_color,False))
        if sign<=0:legend.append(('虧損的極端交易',red,False))
    for t in population:
        color='black'
        if key=='net':
            if data['outlier_upper'] is not None and t[key]>data['outlier_upper']:color=gain_color
            if data['outlier_lower'] is not None and t[key]<data['outlier_lower']:color=red
        g.dot(t['id'],t[key],color,trade_marker)
    if show_open:
        if open_trade:g.dot(open_trade['id'],open_trade[key],'blue',trade_marker)
        legend.append(('未平倉交易','blue',False))
    g.legend(legend+[('平均','black',False,'line')])
    return g.save(name)

paths=[trade_dots('all-trades','net',trades,'損益',show_open=True),
       trade_dots('winning-trades','net',[t for t in trades if t['net']>0],'獲利',1)]

def period_graph(rows,name,mode):
    factors=[(100/abs(r['base']) if r['base'] else 0) if mode=='pct' else 1 for r in rows]
    if mode in ('pnl','pct'):values=[r[k]*f for r,f in zip(rows,factors) for k in ['profit','loss','mfe','mae']]
    elif mode=='cumulative':
        values=[];total=0
        for r in rows:total+=r['pnl'];values.append(total)
    else:values=[r['pnl'] for r in rows]
    g=Graph(values,len(rows),'損益 (%)' if mode=='pct' else '獲利 (¤)' if mode=='cumulative' else '損益 (¤)','月' if mode=='average' else '年' if name.startswith('annual') else '日期')
    if name.startswith('monthly') and mode!='average':
        names=['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];ticks=[]
        for i,r in enumerate(rows):
            year,month=map(int,r['label'].split('-'))
            if month in ([1] if FULL else [1,7]):ticks.append((i,names[month-1]+str(year)))
    else:ticks=list(enumerate(r['label'] for r in rows))
    g.ticks(ticks)
    if mode in ('pnl','pct'):
        for i,(r,f) in enumerate(zip(rows,factors)):
            for key,color in [('mfe','green'),('mae','red')]:g.bar(i,r[key]*f,color,True)
            for key,color in [('profit','forestgreen'),('loss','red')]:g.bar(i,r[key]*f,color)
        g.legend([('獲利','forestgreen',False),('虧損','red',False),('最大可能獲利','green',True),('最大可能虧損','red',True)])
    elif mode=='cumulative':
        points=[(g.x(i),g.y(v)) for i,v in enumerate(values)]
        if len(points)>1:g.draw.line(points,fill='black',width=1)
        for i,v in enumerate(values):g.dot(i,v,'black',3)
        g.legend([('獲利','black',False)])
    else:
        for i,v in enumerate(values):g.bar(i,v,'forestgreen' if v>=0 else 'red')
        g.legend([('獲利','forestgreen',False),('虧損','red',False)])
    return g.save(name)

for name,rows,mode in [('monthly-pnl',data['monthly'],'pnl'),('monthly-pct',data['monthly'],'pct'),('monthly-cumulative',data['monthly'],'cumulative'),('monthly-average',data['average_months'],'average'),('annual-pnl',data['annual'],'pnl'),('annual-pct',data['annual'],'pct')]:
    paths.append(period_graph(rows,name,mode))
if FULL:
    paths.extend([
        trade_dots('losing-trades','net',[t for t in trades if t['net']<0],'虧損',-1),
        trade_dots('mae','mae',excursions,'最大可能虧損'),
        trade_dots('mfe','mfe',excursions,'最大可能獲利'),
    ])
    for name,key in [('drawdown-pnl','mae'),('runup-pnl','mfe')]:
        label='最大可能虧損' if key=='mae' else '最大可能獲利'
        g=trade_graph([0]+[t[k] for t in trades for k in ('net',key)],label+' (¤)')
        zero=g.y(0)
        g.draw.line([(g.left,zero),(g.right,zero)],fill='black')
        layers={color:Image.new('L',g.image.size) for color in (green,red)}
        marks={color:ImageDraw.Draw(layer) for color,layer in layers.items()}
        for t in trades:
            x=round(g.x(t['id']))
            marks[green if t['net']>=0 else red].line([(x,g.y(t[key])),(x,g.y(t['net']))],fill=255)
        for color,mask in layers.items():
            g.image.paste(color,(0,0),mask=mask if len(trades)<1200 else mask.point(lambda x: x*3//4))
        g.legend([('獲利減少' if key=='mae' else '獲利增加',green,False),('虧損減少' if key=='mae' else '虧損增加',red,False)])
        paths.append(g.save(name))
    for key in ['mae','mfe']:
        for pct in [False,True]:
            points=[]
            for t in trades:
                if not t['net']:continue
                notional=abs(t['entry_price']*t['quantity']*data['point_value'])
                if pct and not notional:continue
                factor=100/notional if pct else 1
                points.append((abs(t[key])*factor,abs(t['net'])*factor,t['net']>0))
            xmax=max((x for x,_,_ in points),default=0)*1.035 or (.01 if pct else 1)
            unit=' (%)' if pct else ' (¤)'
            g=Graph([0]+[y for _,y,_ in points],0,'損益'+unit,('最大可能虧損' if key=='mae' else '最大可能獲利')+unit,xmax=xmax,percent=pct)
            g.ticks(axis_ticks(0,xmax,7))
            pixel_points=dict.fromkeys((round(g.x(x)),round(g.y(y)),win) for x,y,win in points)
            for x,y,win in pixel_points:
                g.triangle(g.draw,x,y,green if win else red,win,3 if len(points)>1200 else 4)
            g.legend([('獲利交易',green,False,'up'),('虧損交易',red,False,'down')])
            paths.append(g.save(key+'-scatter'+('-pct' if pct else '')))
    paths.append(period_graph(data['annual'],'annual-cumulative','cumulative'))
(p.parent/'mc-charts.json').write_text(json.dumps(paths),encoding='utf-8')
print(json.dumps({'charts':len(paths),'source':'engine-derived-values-only'}))
