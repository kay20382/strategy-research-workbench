"""Verify persisted metrics and chart grid settings after workbook export."""
import argparse
import importlib.util
import json
from decimal import Decimal
from pathlib import Path
import xml.etree.ElementTree as ET
import zipfile

def main():
    p = argparse.ArgumentParser()
    p.add_argument('workbook', type=Path)
    p.add_argument('--expected-charts', type=int)
    args = p.parse_args()
    native_layout=args.workbook.with_suffix('.layout.json').exists()
    expected_pictures=json.loads(args.workbook.with_suffix('.layout.json').read_text(encoding='utf-8')).get('imageCount',8) if native_layout else 0
    expected_charts=args.expected_charts if args.expected_charts is not None else (0 if native_layout else 8)
    spec = importlib.util.spec_from_file_location('mc_compare', Path(__file__).with_name('compare-mc-report.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    saved = module.read_workbook(args.workbook)
    metrics = json.loads(args.workbook.with_suffix('.metrics.json').read_text(encoding='utf-8'))
    differences = []
    for address, value in metrics['cells'].items():
        actual = saved['策略分析'].get(address)
        if isinstance(value, (int, float)):
            equal = actual is not None and abs(Decimal(actual) - Decimal(str(value))) <= Decimal('0.000000001')
        else:
            equal = actual == value
        if not equal:
            differences.append(dict(cell=address, expected=value, actual=actual))
    with zipfile.ZipFile(args.workbook) as archive:
        bad_member = archive.testzip()
        charts = [n for n in archive.namelist() if '/charts/' in n and Path(n).name.startswith('chart') and n.endswith('.xml')]
        pictures=[n for n in archive.namelist() if n.startswith('xl/media/')]
        ns = {'c': 'http://schemas.openxmlformats.org/drawingml/2006/chart', 'a': 'http://schemas.openxmlformats.org/drawingml/2006/main'}
        visible = []
        for name in charts:
            chart = ET.fromstring(archive.read(name))
            for tag in ['majorGridlines', 'minorGridlines']:
                for grid in chart.findall('.//c:' + tag, ns):
                    if grid.find('.//a:noFill', ns) is None:
                        visible.append(dict(chart=name, grid=tag))
    result = dict(saved_metric_cells_checked=len(metrics['cells']), saved_metric_differences=differences,
                  chart_count=len(charts), picture_count=len(pictures), native_mc_layout=native_layout, visible_chart_gridlines=visible, zip_bad_member=bad_member,
                  workbook_bytes=args.workbook.stat().st_size, provisional_cells=metrics['provisional_cells'])
    args.workbook.with_suffix('.saved-validation.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps(result, ensure_ascii=False))
    return 0 if not differences and len(charts) == expected_charts and (not native_layout or len(pictures)==expected_pictures) and not visible and bad_member is None else 1

if __name__ == '__main__':
    raise SystemExit(main())
