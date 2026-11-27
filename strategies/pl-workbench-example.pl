{ 範例策略，使用既有行情測試程式流程，並非交易建議。 }
Inputs: FastLength(10), SlowLength(30);
Vars: FastMA(0), SlowMA(0);
FastMA = Average(Close, FastLength);
SlowMA = Average(Close, SlowLength);
if MarketPosition = 0 and FastMA > SlowMA then
    Buy ("MAEntry") next bar at market;
if MarketPosition = 1 and FastMA < SlowMA then
    Sell ("MAExit") next bar at market;
