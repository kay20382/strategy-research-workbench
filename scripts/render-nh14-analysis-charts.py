"""依 MC 圖表種類繪製共用引擎策略分析圖，不讀取 MC 績效值。"""
import csv
import hashlib
import json
import math
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont

FONT = 'C:/Windows/Fonts/mingliu.ttc'
GREEN, RED = '#168416', '#ff3030'
TITLES = ['詳細權益曲線', '詳細權益曲線及績效拉回', '多單詳細權益曲線',
          '空單詳細權益曲線', '權益增加 & 拉回', '權益增加 & 拉回(%)',
          '平倉權益曲線', '平倉權益曲線及績效拉回', '買進持有績效', '月增值指數']


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def align_bars(report, source_bars):
    """完整行情可含期間外資料；只取本次含暖機區間，逐根核對時間，不補棒。"""
    times=[datetime.fromisoformat(row['datetime']) for row in report['equity']]
    if not times:raise ValueError('報告沒有權益資料')
    bars=[row for row in source_bars if times[0]<=datetime.fromisoformat(row['datetime'])<=times[-1]]
    if len(bars)!=len(times) or len(times)!=report['bar_count']:raise ValueError('圖表行情根數與本次回測不同')
    if [datetime.fromisoformat(row['datetime']) for row in bars]!=times:raise ValueError('圖表行情時間、順序或重複棒與本次回測不同')
    return bars


def replay(report, bars):
    """獨立逐價重播帳本，保留成交前後費用與側別棒內極值。"""
    fills = defaultdict(list)
    for fill in report['fills']:
        fills[fill['bar']].append(fill)
    initial = report['config']['initial_capital']
    value = report['config']['point_value']
    books = {1: 0., -1: 0.}
    side_peaks = {1: 0., -1: 0.}
    side_drawdowns = {1: 0., -1: 0.}
    side_drawdown_percents = {1: 0., -1: 0.}
    position = None
    peak = initial
    result = []
    for index, bar in enumerate(bars):
        lows = {1: float('inf'), -1: float('inf')}
        highs = {1: -float('inf'), -1: -float('inf')}
        low, high, dd, ddpct = float('inf'), -float('inf'), 0., 0.

        def mark(price):
            nonlocal peak, low, high, dd, ddpct
            sides = dict(books)
            if position:
                entry, side, qty = position
                sides[side] += (price-entry)*side*qty*value
            total = initial + sum(sides.values())
            peak = max(peak, total)
            low, high = min(low, total), max(high, total)
            dd = min(dd, total-peak)
            ddpct = min(ddpct, 100*(total-peak)/peak if peak else 0)
            for side in (1, -1):
                side_peaks[side] = max(side_peaks[side], sides[side])
                side_drawdowns[side] = min(side_drawdowns[side], sides[side]-side_peaks[side])
                capital_peak = initial+side_peaks[side]
                if capital_peak > 0:
                    side_drawdown_percents[side] = min(side_drawdown_percents[side], 100*(sides[side]-side_peaks[side])/capital_peak)
                lows[side] = min(lows[side], sides[side])
                highs[side] = max(highs[side], sides[side])
            return total

        def execute(fill):
            nonlocal position
            price = fill['price']
            mark(price)
            if fill['is_entry']:
                assert position is None, (index, '重疊進場')
                side = fill['side']
                position = (price, side, fill['quantity'])
            else:
                assert position is not None, (index, '無部位平倉')
                entry, side, qty = position
                books[side] += (price-entry)*side*qty*value
                position = None
            books[side] -= fill['fee']
            mark(price)

        o, h, l, c = (float(bar[key]) for key in ('open', 'high', 'low', 'close'))
        pending = list(fills[index])
        close_fills = [fill for fill in pending if fill['phase'] == 'this_bar_close']
        pending = [fill for fill in pending if fill['phase'] != 'this_bar_close']
        mark(o)
        while pending and pending[0]['phase'] == 'open':
            execute(pending.pop(0))
        mark(o)
        current = o
        for end in ([h, l, c] if abs(h-o) < abs(o-l) else [l, h, c]):
            while pending and min(current, end) <= pending[0]['price'] <= max(current, end):
                fill = pending.pop(0)
                current = fill['price']
                execute(fill)
            current = end
            mark(end)
        assert not pending, (index, '未重播成交')
        for fill in close_fills:
            execute(fill)
        close = mark(c)
        expected = report['equity'][index]
        assert abs(close-expected['equity']) < 1e-6, (index, close, expected['equity'])
        assert abs(low-expected['equity_low']) < 1e-6, (index, '低權益')
        assert abs(high-expected['equity_high']) < 1e-6, (index, '高權益')
        result.append({'long_low': lows[1], 'long_high': highs[1],
                       'short_low': lows[-1], 'short_high': highs[-1],
                       'long_drawdown': side_drawdowns[1], 'short_drawdown': side_drawdowns[-1],
                       'long_drawdown_percent': side_drawdown_percents[1], 'short_drawdown_percent': side_drawdown_percents[-1],
                       'dd': dd, 'ddpct': ddpct})
    return result


def one_side_extremes(report):
    """When just one side is active per bar, the inactive side's realized P&L is constant."""
    exposure = defaultdict(set)
    for fill in report['fills']:
        exposure[fill['bar']].add(fill['side'] if fill['is_entry'] else -fill['side'])
    settlements = defaultdict(list)
    for trade in report['trades']:
        settlements[trade['exit_bar']].append(trade)
    capital = report['config']['initial_capital']
    realized = {1: 0., -1: 0.}
    previous_position = 0
    result = []
    for index, row in enumerate(report['equity']):
        sides = exposure.get(index, set()).copy()
        for position in (previous_position, row['position']):
            if position:
                sides.add(1 if position > 0 else -1)
        if len(sides) > 1:
            return None
        values = {}
        for side, label in ((1, 'long'), (-1, 'short')):
            for edge in ('high', 'low'):
                values[f'{label}_{edge}'] = (
                    row[f'equity_{edge}'] - capital - realized[-side] if side in sides else realized[side]
                )
        for trade in settlements.get(index, ()):
            realized[trade['side']] += trade['net']
        previous_position = row['position']
        if not previous_position and not math.isclose(row['equity'], capital + sum(realized.values()), abs_tol=1e-6, rel_tol=0):
            return None
        result.append(values)
    return result


class Figure:
    def __init__(self):
        self.image = Image.new('RGB', (800, 800), 'white')
        self.draw = ImageDraw.Draw(self.image)
        self.font = ImageFont.truetype(FONT, 21)
        self.small = ImageFont.truetype(FONT, 17)

    def legend(self, items):
        widths = [self.draw.textlength(t, font=self.small)+48 for t, _ in items]
        x = 400-sum(widths)/2
        for (label, color), width in zip(items, widths):
            self.draw.line((x, 775, x+28, 775), fill=color, width=2)
            self.draw.text((x+33, 775), label, font=self.small, fill='black', anchor='lm')
            x += width

    def panel(self, times, series, ylabel, rect=(172, 24, 775, 678), percent=False, area=False, dates=True, gap_days=14, hatch=False):
        left, top, right, bottom = rect
        values = [v for _, vals in series for v in vals]
        lo, hi = min(values+[0]), max(values+[0])
        if lo == hi:
            hi = lo+1
        span = hi-lo
        lo, hi = lo-span*.035, hi+span*.035
        raw = (hi-lo)/(6 if bottom-top > 300 else 3)
        scale = 10**math.floor(math.log10(raw))
        step = next(v*scale for v in (1, 2, 2.5, 5, 10) if v*scale >= raw)
        ts = [t.timestamp() for t in times]
        t0, t1 = ts[0], ts[-1]
        def x(i):
            return left+3+(ts[i]-t0)/max(t1-t0, 1)*(right-left-6)
        def y(v):
            return bottom-(v-lo)/(hi-lo)*(bottom-top)
        def dash(a, b):
            x0, y0 = a
            x1, y1 = b
            n = max(abs(x1-x0), abs(y1-y0))
            for k in range(0, int(n), 7):
                u, v = k/n, min(k+4, n)/n
                self.draw.line((x0+(x1-x0)*u, y0+(y1-y0)*u,
                                x0+(x1-x0)*v, y0+(y1-y0)*v), fill='#888888')
        at = math.ceil(lo/step)*step
        while at <= hi:
            yy = y(at)
            dash((left, yy), (right, yy))
            label = f'{at:,.1f}' if percent and step < 1 else f'{at:,.0f}'
            self.draw.text((left-12, yy), label, font=self.font, fill='black', anchor='rm')
            at += step
        self.draw.line((left, top, left, bottom, right, bottom), fill='black')
        label_image = Image.new('RGBA', (310, 30), (255, 255, 255, 0))
        ImageDraw.Draw(label_image).text((155, 15), ylabel, font=self.font, fill='black', anchor='mm')
        rotated = label_image.rotate(90, expand=True)
        self.image.paste(rotated, (21, int((top+bottom)/2-155)), rotated)
        for frac in (0, .5, 1):
            i = min(range(len(ts)), key=lambda j: abs(ts[j]-(t0+(t1-t0)*frac)))
            xx = x(i)
            dash((xx, top), (xx, bottom))
            if dates:
                anchor = 'lm' if frac == 0 else 'rm' if frac == 1 else 'mm'
                self.draw.text((xx, bottom+25), times[i].strftime('%Y/%m/%d'), font=self.small, fill='black', anchor=anchor)
        # 每像素保留極值，避免細密棒數消失；資料缺口不補值或連線。
        for color, vals in series:
            if area and hatch:
                mask = Image.new('L', (800, 800), 0)
                md = ImageDraw.Draw(mask)
                segment = []
                for i, val in enumerate(vals):
                    if segment and ts[i]-ts[i-1] > gap_days*86400:
                        md.polygon([(segment[0][0], y(0))]+segment+[(segment[-1][0], y(0))], fill=255)
                        segment = []
                    segment.append((x(i), y(val)))
                if segment:
                    md.polygon([(segment[0][0], y(0))]+segment+[(segment[-1][0], y(0))], fill=255)
                layer = Image.new('RGBA', (800, 800), (0, 0, 0, 0))
                ld = ImageDraw.Draw(layer)
                for k in range(-800, 801, 8):
                    ld.line((k, 800, k+800, 0), fill=color)
                self.image.paste(layer, (0, 0), ImageChops.multiply(layer.getchannel('A'), mask))
            groups = []
            for i, val in enumerate(vals):
                px = round(x(i))
                if groups and px == groups[-1][0] and ts[i]-ts[groups[-1][3]] <= gap_days*86400:
                    old = groups[-1]
                    groups[-1] = (px, min(old[1], val), max(old[2], val), i)
                else:
                    groups.append((px, val, val, i))
            previous = None
            for px, low, high, i in groups:
                if area and not hatch:
                    self.draw.line((px, y(0), px, y(low if abs(low)>abs(high) else high)), fill=color)
                self.draw.line((px, y(low), px, y(high)), fill=color)
                if previous and ts[i]-ts[previous[2]] <= gap_days*86400:
                    self.draw.line((previous[0], y(previous[1]), px, y(vals[i])), fill=color)
                previous = (px, vals[i], i)
        if dates:
            self.draw.text(((left+right)/2, bottom+60), '日期', font=self.font, fill='black', anchor='mm')


def main():
    if len(sys.argv) not in (4, 5):
        raise SystemExit('用法：render-nh14-analysis-charts.py report.json bars.csv 新輸出目錄 [--transport|--closed-only]')
    report_path, bars_path, out = map(Path, sys.argv[1:4])
    mode = sys.argv[4] if len(sys.argv) == 5 else ''
    if out.exists():
        raise SystemExit('輸出目錄已存在，禁止覆寫：'+str(out))
    r = json.loads(report_path.read_text(encoding='utf-8'))
    bars, source_bars, replayed = [], [], None
    if mode != '--closed-only':
        with bars_path.open(encoding='utf-8-sig', newline='') as stream:
            source_bars = list(csv.DictReader(stream))
        bars = align_bars(r, source_bars)
        if mode != '--transport':
            assert r['config']['intrabar_policy'] == 'nearest_extreme'
            replayed = replay(r, bars)
        elif r['config']['intrabar_policy'] == 'nearest_extreme':
            try:
                replayed = replay(r, bars)
            except AssertionError as error:
                print(f'多空逐價重播未通過核對 {ascii(error)}')
    start = r['config']['warmup_bars']
    initial = r['config']['initial_capital']
    equity = r['equity'][start:]
    side_extremes = replayed if replayed or mode != '--transport' else one_side_extremes(r)
    side_rows = side_extremes[start:] if side_extremes else []
    if not equity:
        raise ValueError('暖機後沒有權益資料')
    times = [datetime.fromisoformat(row['datetime']) for row in equity]
    high = [row['equity_high']-initial for row in equity]
    low = [row['equity_low']-initial for row in equity]
    dd = [row['dd'] for row in side_rows] if replayed else [row['bar_drawdown'] for row in equity]
    if replayed:
        ddpct = [row['ddpct'] for row in side_rows]
    else:
        ddpct = []
        equity_peak = initial
        for row in equity:
            equity_peak = max(equity_peak, row['equity_high'])
            ddpct.append(100 * row['bar_drawdown'] / equity_peak if equity_peak else 0)
    trades_by_bar = defaultdict(list)
    for trade in r['trades']:
        trades_by_bar[trade['exit_bar']].append(trade)
    total = 0.
    closed, closeddd, closedpct = [], [], []
    peak = initial
    for i in range(start, len(r['equity'])):
        total += sum(t['net'] for t in trades_by_bar[i])
        peak = max(peak, initial+total)
        closed.append(total)
        closeddd.append(initial+total-peak)
        closedpct.append(100*(initial+total-peak)/peak if peak else 0)
    hold = [(float(b['close'])-float(bars[start]['open']))*r['config']['point_value'] for b in bars[start:]]
    months = {}
    for row in equity:
        months[row['datetime'][:7]] = row
    month_times = [datetime.fromisoformat(v['datetime']) for v in months.values()]
    month_index = [1000*v['equity']/initial for v in months.values()]
    out.mkdir(parents=True)
    numbers = (7, 8) if mode == '--closed-only' else [n for n in range(1, 11) if side_rows or n not in (3, 4)]
    for number in numbers:
        fig = Figure()
        if number in (1, 2):
            lines = [(GREEN, high), (RED, low)]
            legend = [('高權益 (¤)', GREEN), ('低權益 (¤)', RED)]
        elif number in (3, 4):
            side = 'long' if number == 3 else 'short'
            lines = [(GREEN, [v[side+'_high'] for v in side_rows]), (RED, [v[side+'_low'] for v in side_rows])]
            legend = [('高權益 (¤)', GREEN), ('低權益 (¤)', RED)]
        elif number in (5, 6):
            factor = 100/initial if number == 6 else 1
            lines = [(GREEN, [v*factor for v in high]), (RED, [v*factor for v in dd])]
            legend = [('權益增加', GREEN), ('權益拉回', RED)]
        elif number in (7, 8):
            lines = [(GREEN, closed)]
            legend = [('平倉權益 (¤)', GREEN)]
        elif number == 9:
            lines = [(GREEN, hold)]
            legend = [('買進持有 (¤)', GREEN)]
        else:
            lines = [(GREEN, month_index)]
            legend = [('月增值指數', GREEN)]
        if number in (2, 8):
            fig.panel(times, lines, '權益 (¤)', (172, 24, 775, 340), dates=False)
            fig.panel(times, [(RED, dd if number == 2 else closeddd)], '權益拉回 (¤)', (172, 355, 775, 510), area=True, dates=False)
            fig.panel(times, [(RED, ddpct if number == 2 else closedpct)], '績效拉回 (%)', (172, 525, 775, 678), percent=True, area=True)
            legend += [('權益拉回 (¤)', RED), ('最大可能虧損 (%)', RED)]
        else:
            fig.panel(month_times if number == 10 else times, lines,
                      '增值指數' if number == 10 else '增加 & 拉回 (%)' if number == 6 else '增加 & 拉回 (¤)' if number == 5 else '權益 (¤)',
                      percent=number == 6, area=number in (5, 6), hatch=number in (5, 6), gap_days=62 if number == 10 else 14)
        fig.legend(legend)
        fig.image.save(out/f'analysis-{number:02d}.png')
    if mode == '--closed-only':
        return
    all_trades = list(r['trades'])
    if r.get('open_trade'):
        all_trades.append(r['open_trade'])
    buy_hold = {}
    for side, name in [(None,'all'),(1,'long'),(-1,'short')]:
        first = next((t for t in all_trades if side is None or t['side']==side),None)
        buy_hold[name] = r['config']['initial_capital']*(float(bars[-1]['close'])/first['entry_price']-1) if first and first['entry_price']>0 else 'n/a'
    receipt = {
        'report_path': str(report_path.resolve()), 'report_sha256': sha(report_path),
        'bars_path': str(bars_path.resolve()), 'bars_sha256': sha(bars_path),
        'renderer_sha256': sha(Path(__file__)), 'bar_seconds': r['config']['interval_minutes']*60,
        'verified_bars': len(bars), 'side_accounting_close_matches_engine_all_bars': replayed is not None,
        'side_max_drawdown': {'long': replayed[-1]['long_drawdown'], 'short': replayed[-1]['short_drawdown']} if replayed else None,
        'side_max_drawdown_percent': {'long': replayed[-1]['long_drawdown_percent'], 'short': replayed[-1]['short_drawdown_percent']} if replayed else None,
        'buy_hold': buy_hold,
        'source_csv_bars': len(source_bars), 'aligned_by_exact_report_timestamps': True,
        'intrabar_high_low_matches_engine_all_bars': replayed is not None,
        'chart_start': times[0].isoformat(), 'chart_end': times[-1].isoformat(),
        'images': [{'file': f'analysis-{i:02d}.png', 'title': TITLES[i-1]} for i in numbers],
        'limitations': [
            '圖表種類及順序採 MC 版面；所有績效值來自研究引擎，未讀取 MC 圖片數值。',
            '多空高低權益依引擎最近極值 OHLC 路徑及成交前後成本重播；全部棒收盤側別合計及高低權益已核對，仍未證明原始 MC 棒內路徑等價。',
            '權益拉回及百分比取各棒內依序最高權益後的最大拉回；百分比以當時最高總權益為分母。',
            '買進持有為首計算棒開盤至各棒收盤，一口乘每點價值，不含交易成本，尚未驗證 MC 等價。',
            '月增值指數為 1000 乘月末實際總權益除初始資金，不合成缺月，尚未驗證 MC 等價。',
            '行情棒超過十四日、月資料超過六十二日的缺口不連線；密集資料以像素區間保留極值，權益增加與拉回採斜線面積。']}
    if not replayed:
        side_note = ('每棒僅一個方向有部位或成交，多空圖的高低權益以引擎全體高低權益扣除另一側已實現損益。側別回撤仍缺值。'
                     if side_rows else '該次資料無法核對側別棒內高低權益，多空圖與側別回撤缺值。')
        receipt['limitations'][1:3] = [side_note, '全體回撤圖採引擎棒內回撤，百分比以該棒結束時的權益高點為分母。']
    (out/'receipt.json').write_text(json.dumps(receipt, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'out': str(out.resolve()), 'images': len(numbers), 'verified_bars': len(bars)}, ensure_ascii=True))


if __name__ == '__main__':
    main()
