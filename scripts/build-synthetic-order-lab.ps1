param([string]$Directory = 'data/synthetic-order-lab')
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Directory) { throw '人工測試資料目錄已存在，拒絕覆寫。' }
New-Item -ItemType Directory -Path $Directory | Out-Null
$rows = [System.Collections.Generic.List[object]]::new()
$start = [datetime]'2026-10-26T09:01:00'
for ($index = 0; $index -lt 171; $index++) {
    $o=100; $h=100; $l=100; $c=100
    $calculation = $index - 50
    if ($calculation -ge 0 -and $calculation -lt 120) {
        $case = [int][math]::Floor($calculation / 10)+1
        $phase = $calculation % 10+1
        if ($phase -eq 3) {
            switch ($case) {
                1 { $o=110; $h=112; $l=108; $c=111 }
                2 { $o=90; $h=92; $l=88; $c=89 }
                3 { $o=90; $h=94; $l=88; $c=92 }
                4 { $o=110; $h=112; $l=106; $c=108 }
            }
        }
        if ($phase -eq 4) {
            switch ($case) {
                5 { $o=100; $h=106; $l=94; $c=101 }
                6 { $o=100; $h=106; $l=94; $c=101 }
                7 { $o=100; $h=102; $l=98; $c=101 }
                8 { $o=100; $h=102; $l=98; $c=101 }
                9 { $o=100; $h=210; $l=99; $c=100 }
                10 { $o=100; $h=104; $l=96; $c=103 }
                11 { $o=100; $h=104; $l=96; $c=97 }
            }
        }
        if ($phase -eq 5 -and $case -eq 10) { $o=101; $h=103; $l=100; $c=102 }
        if ($phase -eq 5 -and $case -eq 11) { $o=99; $h=100; $l=97; $c=98 }
    }
    if ($index -eq 170) { $o=103; $h=107; $l=102; $c=106 }
    $time = $start.AddMinutes($index)
    $rows.Add([pscustomobject]@{ Date=$time.ToString('yyyy/MM/dd');Time=$time.ToString('HH:mm:ss');Open=$o;High=$h;Low=$l;Close=$c;TotalVolume=1 })
}
$rows | Export-Csv -LiteralPath (Join-Path $Directory 'SYNTHETIC_ORDER_LAB_1MIN.csv') -NoTypeInformation -Encoding utf8NoBOM
# 人工期望值獨立定義，不讀取引擎結果。
# S05/S06僅在「先近端極值，等距先低」假設下固定，MC實際順序另行比對。
$entries = @(110,90,90,110,100,100,100,100,100,100,100,100)
$exits = @(100,100,100,100,95,95,100,100,100,101,99,106)
$sides = @(1,-1,1,-1,1,-1,1,-1,1,1,-1,1)
$expected = foreach ($number in 1..12) {
    $quantity=1; if ($number -eq 12) { $quantity=3 }
    $gross=($exits[$number-1]-$entries[$number-1])*$sides[$number-1]*$quantity
    [pscustomobject]@{case=$number;side=$sides[$number-1];quantity=$quantity;entry=$entries[$number-1];exit=$exits[$number-1];gross=$gross;commission=2*$quantity;net=$gross-2*$quantity}
}
$expected | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $Directory 'manual-expectations.json') -Encoding utf8
