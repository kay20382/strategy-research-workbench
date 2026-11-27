{ 人工行情測試，不是真實市場資料。
  僅搭配 SYNTHETIC_ORDER_LAB_1MIN.csv，1分K，暖機固定50。
  每案10根，共12案；最後額外1根驗證收盤平倉與未執行次棒單。
}
Inputs: CaseBars(10);
Vars: BlockNo(0), CaseNo(0), PhaseNo(0), MP(0);
BlockNo = IntPortion((CurrentBar - 1) / CaseBars);
CaseNo = BlockNo + 1;
PhaseNo = CurrentBar - BlockNo * CaseBars;
MP = MarketPosition;

if CurrentBar <= 120 then begin
    if CaseNo = 1 then begin
        if PhaseNo = 2 then Buy ("S01_GAP_STOP") 1 contract next bar at 105 stop;
        if PhaseNo = 4 and MP = 1 then Sell ("S01_EXIT") next bar at market;
    end;
    if CaseNo = 2 then begin
        if PhaseNo = 2 then SellShort ("S02_GAP_STOP") 1 contract next bar at 95 stop;
        if PhaseNo = 4 and MP = -1 then BuyToCover ("S02_EXIT") next bar at market;
    end;
    if CaseNo = 3 then begin
        if PhaseNo = 2 then Buy ("S03_GAP_LIMIT") 1 contract next bar at 95 limit;
        if PhaseNo = 4 and MP = 1 then Sell ("S03_EXIT") next bar at market;
    end;
    if CaseNo = 4 then begin
        if PhaseNo = 2 then SellShort ("S04_GAP_LIMIT") 1 contract next bar at 105 limit;
        if PhaseNo = 4 and MP = -1 then BuyToCover ("S04_EXIT") next bar at market;
    end;
    if CaseNo = 5 then begin
        if PhaseNo = 2 then Buy ("S05_ENTRY") 1 contract next bar at market;
        if PhaseNo = 3 and MP = 1 then begin
            Sell ("S05_TARGET") next bar at 105 limit;
            Sell ("S05_STOP") next bar at 95 stop;
        end;
    end;
    if CaseNo = 6 then begin
        if PhaseNo = 2 then SellShort ("S06_ENTRY") 1 contract next bar at market;
        if PhaseNo = 3 and MP = -1 then begin
            BuyToCover ("S06_TARGET") next bar at 95 limit;
            BuyToCover ("S06_STOP") next bar at 105 stop;
        end;
    end;
    if CaseNo = 7 then begin
        if PhaseNo = 2 then Buy ("S07_ENTRY") 1 contract next bar at market;
        if PhaseNo = 3 and MP = 1 then begin
            Sell ("S07_LIMIT") next bar at 100 limit;
            Sell ("S07_STOP") next bar at 100 stop;
        end;
    end;
    if CaseNo = 8 then begin
        if PhaseNo = 2 then SellShort ("S08_ENTRY") 1 contract next bar at market;
        if PhaseNo = 3 and MP = -1 then begin
            BuyToCover ("S08_LIMIT") next bar at 100 limit;
            BuyToCover ("S08_STOP") next bar at 100 stop;
            BuyToCover ("S08_MARKET") next bar at market;
        end;
    end;
    if CaseNo = 9 then begin
        if PhaseNo = 2 then Buy ("S09_EXPIRE") 1 contract next bar at 200 stop;
        if PhaseNo = 6 and MP = 0 then Buy ("S09_ENTRY") 1 contract next bar at market;
        if PhaseNo = 8 and MP = 1 then Sell ("S09_EXIT") next bar at market;
    end;
    if CaseNo = 10 then begin
        if PhaseNo = 2 then Buy ("S10_ENTRY") 1 contract next bar at market;
        if (PhaseNo = 3 or PhaseNo = 4) and MP = 1 then begin
            if PhaseNo = 3 then Value1 = 95 else Value1 = 102;
            Sell ("S10_REPRICE") next bar at Value1 stop;
        end;
    end;
    if CaseNo = 11 then begin
        if PhaseNo = 2 then SellShort ("S11_ENTRY") 1 contract next bar at market;
        if (PhaseNo = 3 or PhaseNo = 4) and MP = -1 then begin
            if PhaseNo = 3 then Value2 = 105 else Value2 = 98;
            BuyToCover ("S11_REPRICE") next bar at Value2 stop;
        end;
    end;
    if CaseNo = 12 then begin
        if PhaseNo = 2 then Buy ("S12_ENTRY_Q3") 3 contracts next bar at market;
    end;
    if CaseNo < 12 and PhaseNo = 9 then begin
        if MP = 1 then Sell ("SYNTH_RESET_LONG") next bar at market;
        if MP = -1 then BuyToCover ("SYNTH_RESET_SHORT") next bar at market;
    end;
end;

if CurrentBar = 121 then begin
    if MP = 1 then Sell ("S12_FINAL_CLOSE") this bar on close;
    Buy ("S12_FINAL_PENDING") 1 contract next bar at market;
end;
