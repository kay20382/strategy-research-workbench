using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using Intelligence;
using Package;
using Smart;

internal sealed class TradeGateway
{
    readonly JavaScriptSerializer json = new JavaScriptSerializer();
    readonly ConcurrentQueue<Action> queue = new ConcurrentQueue<Action>();
    readonly Dictionary<long, string> requests = new Dictionary<long, string>();
    readonly Dictionary<string, string> exchangeRequests = new Dictionary<string, string>();
    readonly Dictionary<string, OrderReference> working = new Dictionary<string, OrderReference>();
    readonly HashSet<string> ids = new HashSet<string>(StringComparer.Ordinal);
    readonly HashSet<string> fills = new HashSet<string>(StringComparer.Ordinal);
    readonly HashSet<string> closedOrders = new HashSet<string>(StringComparer.Ordinal);
    readonly HashSet<string> reportTopics = new HashSet<string>(StringComparer.Ordinal);
    readonly Dictionary<string, string> pendingQueries = new Dictionary<string, string>();
    readonly string mode;
    readonly bool foreign;
    readonly bool replay;
    readonly Dictionary<string, OrderReference> replayOrders = new Dictionary<string, OrderReference>();
    readonly Dictionary<string, int> replayPositions = new Dictionary<string, int>();
    readonly TextWriter protocol;
    TaiFexCom sdk;
    string broker, account, trader, group;
    bool ready, loginState, accountMatched, brokerFromLogin, stop, reportReady, reportRecoverySeen, productsRequested;
    DateTime loginDeadline;
    int paperCounter;

    sealed class OrderReference
    {
        public string Key, Symbol, Web, Cnt, Number;
        public SIDE_FLAG Side;
        public PRICE_FLAG PriceFlag;
        public TIME_IN_FORCE Duration;
        public POSITION_EFFECT Effect;
        public decimal Price;
        public ushort Quantity;
    }

    TradeGateway(string mode, bool foreign, bool replay, TextWriter protocol) { this.mode = mode; this.foreign = foreign; this.replay = replay; this.protocol = protocol; }

    static readonly CultureInfo Invariant = CultureInfo.InvariantCulture;
    static string Text(object value) { return Convert.ToString(value, Invariant).Trim(); }
    static string Get(Dictionary<string, object> body, string key)
    {
        object value;
        return body.TryGetValue(key, out value) ? Text(value) : "";
    }
    static string Setting(string key, bool required = true)
    {
        string value = Environment.GetEnvironmentVariable("TRADECOM_" + key);
        if (String.IsNullOrWhiteSpace(value) && required) throw new InvalidOperationException("交易設定缺少 " + key);
        return value ?? "";
    }
    void Emit(string type, object data)
    {
        protocol.WriteLine("TRADE:" + json.Serialize(new { type = type, mode = mode, data = data }));
        protocol.Flush();
    }
    void Status(string state, string message) { Emit("status", new { state = state, message = message }); }
    void Reply(string id, string state, string message)
    {
        Emit("reply", new { id = id, state = state, message = message });
    }
    void SetReady()
    {
        if (ready || !loginState || !accountMatched) return;
        ready = true;
        Status("ready", brokerFromLogin ? "已登入，指定交易帳號已核對。唯讀查詢採用券商回報的分公司代號。" :
            "已登入，指定交易帳號已核對。");
        CompleteRecovery();
    }
    bool Matches(string b, string a, string t)
    {
        return Text(b)==broker && Text(a)==account && (trader=="" || Text(t)==trader);
    }
    static bool Success(string code)
    {
        int value;
        return String.IsNullOrWhiteSpace(code) || (Int32.TryParse(code, out value) && value == 0);
    }

    void Login()
    {
        if (mode=="paper")
        {
            loginState=accountMatched=reportReady=true;
            SetReady();
            return;
        }
        string host=Setting("HOST");
        ushort port=UInt16.Parse(Setting("PORT"),Invariant);
        broker=Setting("BROKER");
        account=Setting("ACCOUNT");
        trader=Setting("SUB_ACCOUNT",false);
        group=Setting("GROUP",false);
        // The SDK uses the signed certificate installed under the current Windows user.
        sdk=new TaiFexCom(host,port,"API",Language.Chinese,false);
        // Keep product downloads separate from the initial order/fill recovery callback.
        sdk.AutoRetriveProductInfo=false;
        sdk.AutoSubReport=!foreign;
        sdk.AutoRecoverReport=!foreign;
        sdk.AutoSubReportForeign=foreign;
        sdk.AutoRecoverReportForeign=foreign;
        sdk.SelfDefineExeName="StrategyStudio";
        sdk.OnGetStatus+=OnStatus;
        sdk.OnRcvMessage+=OnMessage;
        sdk.OnRcvServerTime+=OnServerTime;
        sdk.OnRecoverStatus+=OnRecovery;
        Status("connecting","正在連線凱基交易主機。");
        loginDeadline=DateTime.UtcNow.AddSeconds(25);
        string user=Setting("USER");
        string password=Setting("PASSWORD");
        if(trader!="" && !user.Contains("@")) user=trader+"@"+user+"."+broker+account;
        if(user.IndexOfAny(new[]{',','|'})>=0 || password.IndexOfAny(new[]{',','|'})>=0)
            sdk.LoginDirect(host,port,user,password,' ');
        else
            sdk.LoginDirect(host,port,user+",,"+password);
    }

    void OnServerTime(object sender, DateTime serverTime, int quality) { }

    void OnStatus(object sender, COM_STATUS status, byte[] message)
    {
        byte[] data=message==null ? new byte[0] : (byte[])message.Clone();
        queue.Enqueue(delegate {
            switch(status)
            {
                case COM_STATUS.CONNECT_READY: Status("connected","交易主機已連線。"); break;
                case COM_STATUS.LOGIN_READY: loginState=true; SetReady(); break;
                case COM_STATUS.NOVALIDCERT: Stop("本機沒有這個登入帳號的有效凱基憑證。"); break;
                case COM_STATUS.NOCGCSPIAPI: Stop("凱基簽章元件未安裝或未註冊。"); break;
                case COM_STATUS.CONNECT_FAIL:
                case COM_STATUS.DISCONNECTED:
                    ready=false;
                    stop=true;
                    Status("disconnected","凱基交易連線已中斷（"+status+"）。"); break;
                case COM_STATUS.LOGIN_FAIL:
                case COM_STATUS.LOGIN_UNKNOW:
                    Stop("凱基交易登入失敗（"+status+"）。"); break;
                case COM_STATUS.ACK_REQUESTID:
                    if(data.Length>=9)
                    {
                        long request=BitConverter.ToInt64(data,0);
                        string id;
                        if(requests.TryGetValue(request,out id))
                            Emit("transport_ack",new { id=id, request_id=Text(request), received=data[8]==1 });
                    }
                    break;
            }
        });
    }

    void OnRecovery(object sender, string topic, RECOVER_STATUS status, uint count)
    {
        queue.Enqueue(delegate {
            if(productsRequested) return;
            if(status==RECOVER_STATUS.RS_BEGIN)
            {
                reportTopics.Add(topic);
                reportReady=false;
            }
            else if(status==RECOVER_STATUS.RS_DONE)
            {
                reportTopics.Remove(topic);
                reportRecoverySeen=true;
                CompleteRecovery();
            }
        });
    }

    void CompleteRecovery()
    {
        if(!ready || reportReady || !reportRecoverySeen || reportTopics.Count!=0) return;
        reportReady=true;
        Emit("recovery",new { state="complete" });
        if(mode=="live")
        {
            productsRequested=true;
            if(foreign) sdk.RetriveForeignProductDetail();
            else { sdk.RetriveProductDetail(); sdk.RetriveProductDetailT1(); }
        }
    }

    void OnMessage(object sender, PackageBase package)
    {
        queue.Enqueue(delegate {
            switch((DT)package.DT)
            {
                case DT.LOGIN:
                    var login=(P001503)package;
                    if(login.Code!=0) { Stop("凱基登入錯誤碼 "+login.Code); break; }
                    var accounts=login.p001503_2??new P001503_2[0];
                    var entry=accounts.FirstOrDefault(a=>Matches(a.BrokeId,a.Account,a.TRADER));
                    if(entry==null && mode=="read_only")
                    {
                        // Broker routing may differ from the manually saved value. Only use a unique,
                        // exact account and sub-account match for read-only requests in this session.
                        var matching=accounts.Where(a=>Text(a.Account)==account && Text(a.TRADER)==trader && Text(a.BrokeId)!="")
                            .Take(2).ToArray();
                        if(matching.Length==1)
                        {
                            entry=matching[0];
                            broker=Text(entry.BrokeId);
                            brokerFromLogin=true;
                        }
                    }
                    if(entry==null) { Stop("凱基已回覆登入成功，但無法核對設定的分公司、交易帳號或子帳。"); break; }
                    if(group=="") group=Text(entry.GROUP);
                    accountMatched=true;
                    SetReady();
                    break;
                case DT.INVENTORY_TRADER:
                    var position=(P001616)package;
                    var positions=position.p001616_2??new P001616_2[0];
                    var rows=positions.Where(p=>Matches(p.BrokerId,p.Account,p.Trader))
                        .Select(p=>new { symbol=p.ComID, maturity=p.ComYM, side=p.BS, quantity=p.OTQty,
                            average=p.DealPrice, market_price=p.MPrice, unrealized=p.PRTLOS, currency=p.Currency }).ToArray();
                    CompleteQuery("positions",position.Code,rows,positions.Length);
                    break;
                case DT.FINANCIAL_TRADERN:
                    var margin=(P001626)package;
                    var marginRows=margin.p001626_2??new P001626_2[0];
                    var balances=marginRows.Where(p=>Matches(p.BrokerId,p.Account,p.Trader))
                        .Select(p=>new { currency=p.Currency, equity=p.EQUITY, available=p.EXCESS,
                            initial_margin=p.IAMT, maintenance=p.MAMT, margin_in_orders=p.ORDAMT }).ToArray();
                    CompleteQuery("margin",margin.Code,balances,marginRows.Length);
                    break;
                case DT.FUT_ORDER_ACK:
                    var ack=(PT02002)package;
                    string client;
                    if(!requests.TryGetValue(ack.RequestId,out client)) break;
                    if(ack.ErrorCode==0 && Text(ack.CNT)!="")
                        exchangeRequests[Text(ack.WEBID)+"|"+Text(ack.CNT)]=client;
                    Emit("order_ack",new { id=client, request_id=Text(ack.RequestId), code=ack.ErrorCode, web_id=ack.WEBID, cnt=ack.CNT });
                    break;
                case DT.FUT_ORDER_RPT:
                    var report=(PT02010)package;
                    if(Matches(report.BrokerId,report.Account,report.Trader))
                        ReadOrderReport(report);
                    break;
                case DT.FUT_DEAL_RPT:
                    var fill=(PT02011)package;
                    if(!Matches(fill.BrokerId,fill.Account,fill.Trader)) break;
                    string exchange=Text(fill.WebID)+"|"+Text(fill.CNT);
                    string fillKey=Text(fill.TradeDate)+"|"+exchange+"|"+Text(fill.MarketNo);
                    if(Text(fill.MarketNo)!="" && !fills.Add(fillKey)) break;
                    exchangeRequests.TryGetValue(exchange,out client);
                    string orderKey=Text(fill.TradeDate)+"|"+exchange;
                    ushort remaining;
                    if(UInt16.TryParse(fill.LeaveQty,out remaining))
                    {
                        if(remaining==0)
                        {
                            closedOrders.Add(orderKey);
                            working.Remove(orderKey);
                        }
                        else
                        {
                            OrderReference openOrder;
                            if(working.TryGetValue(orderKey,out openOrder))
                                openOrder.Quantity=remaining;
                        }
                    }
                    Emit("fill",new { id=client, symbol=fill.Symbol, trade_date=fill.TradeDate, time=fill.ReportTime,
                        report_time2=Text(fill.ReportTime2), session=Text(fill.Session), side=Text(fill.Side),
                        received_at=DateTime.UtcNow.AddHours(8).ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff",Invariant),
                        recovered=!reportReady,
                        order_no=fill.OrderNo, web_id=fill.WebID, cnt=fill.CNT, execution_id=fill.MarketNo,
                        price=fill.DealPrice, quantity=fill.DealQty, cumulative=fill.CumQty, remaining=fill.LeaveQty });
                    break;
                case DT.FFUT_ORDER_ACK:
                    if(!foreign) break;
                    var foreignAck=(PT03302)package;
                    if(!requests.TryGetValue(foreignAck.RequestId,out client)) break;
                    if(foreignAck.ErrCode==0 && Text(foreignAck.CNT)!="")
                        exchangeRequests[Text(foreignAck.WEBID)+"|"+Text(foreignAck.CNT)]=client;
                    Emit("order_ack",new { id=client, market="foreign", request_id=Text(foreignAck.RequestId),
                        code=foreignAck.ErrCode, web_id=foreignAck.WEBID, cnt=foreignAck.CNT });
                    break;
                case DT.FFUT_ORDER_RPT:
                    var foreignReport=(PT03310)package;
                    if(!foreign || foreignReport.CP!='F' || !Matches(foreignReport.BrokerId,foreignReport.Account,foreignReport.AE)) break;
                    exchangeRequests.TryGetValue(Text(foreignReport.WEBID)+"|"+Text(foreignReport.CNT),out client);
                    Emit("order_report",new { id=client, market="foreign", exchange=foreignReport.Exchange,
                        product=foreignReport.Symbol, maturity=foreignReport.ComYM,
                        symbol=ForeignSymbol(foreignReport.Exchange,foreignReport.Symbol,foreignReport.ComYM),
                        code=foreignReport.ErrCode, function=Text(foreignReport.OrderFunc), order_no=foreignReport.ASorderNo,
                        web_id=foreignReport.WEBID, cnt=foreignReport.CNT, trade_date=foreignReport.TradeDate,
                        time=foreignReport.ReportTime, side=Text(foreignReport.BS), price=foreignReport.Price, quantity=foreignReport.AfterQty,
                        recovered=!reportReady });
                    break;
                case DT.FFUT_DEAL_RPT:
                    var foreignFill=(PT03311)package;
                    if(!foreign || foreignFill.CP!='F' || !Matches(foreignFill.BrokerId,foreignFill.Account,foreignFill.AE)) break;
                    string foreignKey=Text(foreignFill.TradeDate)+"|"+Text(foreignFill.Exchange)+"|"+Text(foreignFill.Symbol)+"|"+
                        Text(foreignFill.ComYM)+"|"+Text(foreignFill.WEBID)+"|"+Text(foreignFill.CNT)+"|"+Text(foreignFill.PATSNo);
                    if(Text(foreignFill.PATSNo)!="" && !fills.Add(foreignKey)) break;
                    exchangeRequests.TryGetValue(Text(foreignFill.WEBID)+"|"+Text(foreignFill.CNT),out client);
                    Emit("fill",new { id=client, market="foreign", exchange=foreignFill.Exchange,
                        product=foreignFill.Symbol, maturity=foreignFill.ComYM,
                        symbol=ForeignSymbol(foreignFill.Exchange,foreignFill.Symbol,foreignFill.ComYM), trade_date=foreignFill.TradeDate,
                        deal_date=foreignFill.DealDate, time=foreignFill.ReportTime, side=Text(foreignFill.BS),
                        recovered=!reportReady, order_no=foreignFill.ASorderNo, web_id=foreignFill.WEBID, cnt=foreignFill.CNT,
                        execution_id=foreignFill.PATSNo, price=foreignFill.DealPrice, quantity=foreignFill.DealQty,
                        cumulative=foreignFill.CumQty, remaining=foreignFill.LeavesQty });
                    break;
            }
        });
    }

    void ReadOrderReport(PT02010 report)
    {
        string exchange=Text(report.WebID)+"|"+Text(report.CNT), client;
        exchangeRequests.TryGetValue(exchange,out client);
        string key=Text(report.TradeDate)+"|"+exchange;
        Emit("order_report",new { id=client, reference=key, symbol=report.Symbol, code=report.Code,
            function=report.OrderFunc.ToString(), order_no=report.OrderNo, web_id=report.WebID, cnt=report.CNT,
            trade_date=report.TradeDate, time=report.ReportTime, side=Text(report.Side), price=report.Price, quantity=report.AfterQty });
        if(Text(report.Market)!="1" || Text(report.Trader)!=trader || !Success(report.Code) || closedOrders.Contains(key)) return;
        ushort qty;
        if(!UInt16.TryParse(report.AfterQty,out qty)) return;
        if(qty==0 || report.OrderFunc=='D') { closedOrders.Add(key); working.Remove(key); return; }
        decimal price;
        if(!Decimal.TryParse(report.Price,NumberStyles.Number,Invariant,out price) || report.PriceMark!='2') return;
        if(report.Side!='B' && report.Side!='S') return;
        if(report.TimeInForce!='R' && report.TimeInForce!='F' && report.TimeInForce!='I') return;
        POSITION_EFFECT effect;
        switch(report.PositionEffect)
        {
            case 'O': case '0': effect=POSITION_EFFECT.PE_OPEN; break;
            case 'C': case '1': effect=POSITION_EFFECT.PE_CLOSE; break;
            case 'T': case '2': effect=POSITION_EFFECT.PE_DAY_TRADE; break;
            default: effect=POSITION_EFFECT.PE_AUTO; break;
        }
        working[key]=new OrderReference { Key=key, Symbol=Text(report.Symbol), Web=Text(report.WebID), Cnt=Text(report.CNT),
            Number=Text(report.OrderNo), Price=price, Quantity=qty, PriceFlag=PRICE_FLAG.PF_SPECIFIED,
            Side=report.Side=='B'?SIDE_FLAG.SF_BUY:SIDE_FLAG.SF_SELL, Effect=effect,
            Duration=report.TimeInForce=='R'?TIME_IN_FORCE.TIF_ROD:report.TimeInForce=='F'?TIME_IN_FORCE.TIF_FOK:TIME_IN_FORCE.TIF_IOC };
    }

    void CompleteQuery(string action, int code, object[] rows, int? receivedRows=null)
    {
        string id;
        if(!pendingQueries.TryGetValue(action,out id)) return;
        pendingQueries.Remove(action);
        Emit("query",new { id=id, action=action, code=code, rows=rows, received_rows=receivedRows??rows.Length });
    }

    void Handle(string line)
    {
        string id="";
        try
        {
            if(line.Length>16384) throw new InvalidOperationException("交易指令太長。");
            var body=json.Deserialize<Dictionary<string,object>>(line);
            id=Get(body,"id");
            string action=Get(body,"action");
            if(!Regex.IsMatch(id,@"^[a-zA-Z0-9_-]{1,64}$")) throw new InvalidOperationException("交易指令缺少有效編號。");
            if(!ids.Add(id)) throw new InvalidOperationException("此指令編號已使用。");
            if(!ready) throw new InvalidOperationException("交易連線尚未就緒。");
            switch(action)
            {
                case "account":
                    Emit("query",new { id=id, action=action, code=0, rows=new[]{new { matched=true, has_sub_account=!String.IsNullOrEmpty(trader),
                        broker_from_login=brokerFromLogin, report_ready=reportReady }} });
                    return;
                case "positions": case "margin":
                    if(pendingQueries.ContainsKey(action)) throw new InvalidOperationException("同類查詢正在等待券商回覆。");
                    pendingQueries.Add(action,id);
                    if(mode=="paper") {
                        object[] positions=replay && action=="positions" ?
                            replayPositions.Where(pair=>pair.Value!=0).Select(pair=>(object)new { symbol=pair.Key, signed_quantity=pair.Value }).ToArray() : new object[0];
                        CompleteQuery(action,0,positions); return;
                    }
                    string market=foreign?"O":"I";
                    long code=action=="positions"?sdk.RetrivePositionSum(market,broker,account,trader,group):sdk.RetriveFMargin(market,broker,account,trader,group);
                    if(code!=0) { pendingQueries.Remove(action); Reply(id,"rejected","查詢回傳碼 "+code); }
                    return;
                case "orders":
                    if(replay) {
                        Emit("query",new { id=id, action=action, code=0, rows=replayOrders.Keys.ToArray() });
                        return;
                    }
                    if(foreign) throw new InvalidOperationException("海外委託請從交易回報或券商平台核對。");
                    Emit("query",new { id=id, action=action, code=0, rows=working.Values.Select(o=>new { reference=o.Key, symbol=o.Symbol,
                        order_no=o.Number, price=Text(o.Price), quantity=o.Quantity, side=o.Side==SIDE_FLAG.SF_BUY?"buy":"sell" }), report_ready=reportReady });
                    return;
                case "place": case "cancel": case "modify_price": case "modify_quantity":
                    if(mode=="read_only") throw new InvalidOperationException("目前連線為唯讀。");
                    if(mode=="live" && (!reportReady || !body.ContainsKey("confirmed") || !Object.Equals(body["confirmed"],true)))
                        throw new InvalidOperationException("正式委託需要完成回報回補並確認委託內容。");
                    if(foreign)
                    {
                        if(action!="place") throw new InvalidOperationException("海外改撤單請使用券商交易平台。");
                        SendForeignOrder(id,body);
                    }
                    else
                    {
                        if(Get(body,"exchange")!="") throw new InvalidOperationException("海外委託須使用海外交易連線。");
                        SendOrder(id,action,body);
                    }
                    return;
                case "replay_execution": case "replay_close_order":
                    if(!replay || mode!="paper") throw new InvalidOperationException("模擬撮合指令僅限海外回放。");
                    CompleteReplayOrder(id,action,body);
                    return;
                default: throw new InvalidOperationException("未知交易指令。");
            }
        }
        catch(InvalidOperationException error) { Reply(id,"rejected",error.Message); }
        catch(Exception) { Reply(id,"unknown","交易指令處理中斷。請核對券商狀態後再處理。"); }
    }

    static string ForeignSymbol(string exchange,string product,string maturity)
    {
        return Text(exchange)+":"+Text(product)+":"+Text(maturity);
    }

    void CompleteReplayOrder(string id,string action,Dictionary<string,object> body)
    {
        string reference=Get(body,"reference");
        OrderReference order;
        if(!replayOrders.TryGetValue(reference,out order)) throw new InvalidOperationException("模擬委託不存在或已結束。");
        if(action=="replay_close_order")
        {
            replayOrders.Remove(reference);
            Emit("order_report",new { id=id, reference=reference, symbol=order.Symbol, market="foreign", state="closed",
                reason=Get(body,"reason"), quantity=0, simulated=true });
            return;
        }
        decimal price;
        if(!Decimal.TryParse(Get(body,"price"),NumberStyles.AllowDecimalPoint|NumberStyles.AllowLeadingSign,Invariant,out price))
            throw new InvalidOperationException("模擬成交價格式錯誤。");
        ushort quantity=Quantity(body);
        if(quantity!=order.Quantity) throw new InvalidOperationException("模擬成交口數與委託不符。");
        int position;
        replayPositions.TryGetValue(order.Symbol,out position);
        int delta=order.Side==SIDE_FLAG.SF_BUY ? quantity : -quantity;
        if((order.Effect==POSITION_EFFECT.PE_OPEN && (position!=0 || replayPositions.Any(pair=>pair.Value!=0))) ||
           (order.Effect==POSITION_EFFECT.PE_CLOSE && position!=-delta))
            throw new InvalidOperationException("模擬成交與帳戶部位不符。");
        replayPositions[order.Symbol]=position+delta;
        replayOrders.Remove(reference);
        Emit("fill",new { id=id, reference=reference, order_no=reference, execution_id=id, market="foreign", symbol=order.Symbol,
            price=Text(price), quantity=quantity, side=delta>0?"buy":"sell", remaining=0,
            time=Get(body,"time"), label=Get(body,"label"), simulated=true });
    }

    void SendForeignOrder(string id,Dictionary<string,object> body)
    {
        string exchange=Get(body,"exchange").ToUpperInvariant(), product=Get(body,"product").ToUpperInvariant();
        string maturity=Get(body,"maturity"), side=Get(body,"side"), effect=Get(body,"position_effect");
        if(!Regex.IsMatch(exchange,@"^[A-Z0-9_]{1,8}$") || !Regex.IsMatch(product,@"^[A-Z0-9]{2,29}$") ||
           !Regex.IsMatch(maturity,@"^20[0-9]{2}(0[1-9]|1[0-2])$"))
            throw new InvalidOperationException("請依凱基商品檔填入交易所、海外期貨商品代碼與 YYYYMM。");
        if(side!="buy" && side!="sell") throw new InvalidOperationException("請指定買進或賣出。");
        if(effect!="open" && effect!="close") throw new InvalidOperationException("請指定新倉或平倉。");
        SIDE_FLAG bs=side=="buy"?SIDE_FLAG.SF_BUY:SIDE_FLAG.SF_SELL;
        POSITION_EFFECT pe=effect=="open"?POSITION_EFFECT.PE_OPEN:POSITION_EFFECT.PE_CLOSE;
        string duration=Get(body,"time_in_force");
        if(duration!="ioc" && duration!="fok" && duration!="rod") throw new InvalidOperationException("請選擇 IOC、FOK 或 ROD。");
        TIME_IN_FORCE tif=duration=="ioc"?TIME_IN_FORCE.TIF_IOC:duration=="fok"?TIME_IN_FORCE.TIF_FOK:TIME_IN_FORCE.TIF_ROD;
        string priceType=Get(body,"price_type");
        if(priceType!="market" && priceType!="limit" && !(replay && (priceType=="stop" || priceType=="close")))
            throw new InvalidOperationException("海外單式期貨目前接受市價或限價。");
        if(priceType=="market" && duration=="rod") throw new InvalidOperationException("市價請使用 IOC 或 FOK。");
        PRICE_FLAG priceFlag=priceType=="market"?PRICE_FLAG.PF_MARKET:PRICE_FLAG.PF_SPECIFIED;
        decimal price=priceType=="limit" || priceType=="stop"?Price(body):0;
        ushort quantity=Quantity(body);
        string symbol=ForeignSymbol(exchange,product,maturity);
        if(mode=="paper")
        {
            if(replay) replayOrders.Add(id,new OrderReference {Symbol=symbol,Side=bs,Effect=pe,Quantity=quantity});
            Emit("order_report",new { id=id, market="foreign", exchange=exchange, product=product, maturity=maturity,
                symbol=symbol, code=0, order_no="paper-"+(++paperCounter), side=side, price=Text(price), quantity=quantity, simulated=true });
            return;
        }
        var months=sdk.GetComYM(MARKET_FLAG.MF_FUT,exchange,product);
        if(sdk.GetProductBase(MARKET_KIND.MK_FOREIGN,exchange,product,MARKET_FLAG.MF_FUT)==null ||
            months==null || !months.Any(month=>Text(month)==maturity))
            throw new InvalidOperationException("凱基海外商品檔尚未確認所填的交易所、商品與到期年月。");
        long request=sdk.GetRequestId();
        requests.Add(request,id);
        Emit("order_dispatch",new { id=id, market="foreign", symbol=symbol, request_id=Text(request) });
        long result=sdk.FOrder(ORDER_KIND.OK_CONTROL,ORDER_TYPE.OT_NEW,request,MARKET_FLAG.MF_FUT,broker,account,trader,
            exchange,product,maturity,0,'F',bs,priceFlag,'N',pe,tif,price,0,quantity,"","","","","","","","","");
        if(result==0) Reply(id,"submitted","SDK 已送出海外期貨委託，等待凱基回報。");
        else Reply(id,"rejected","海外委託 SDK 回傳碼 "+result);
    }

    OrderReference NewOrder(Dictionary<string,object> body)
    {
        string product=Get(body,"product").ToUpperInvariant(), month=Get(body,"maturity"), side=Get(body,"side");
        if(!Regex.IsMatch(product,@"^[A-Z][A-Z0-9]{1,7}$") || !Regex.IsMatch(month,@"^20[0-9]{2}(0[1-9]|1[0-2])$"))
            throw new InvalidOperationException("請填入期貨簡碼及 YYYYMM 到期月份。");
        if(side!="buy" && side!="sell") throw new InvalidOperationException("請指定買進或賣出。");
        string symbol=mode=="paper"?product+"-"+month:sdk.GenFutSymbol(product,month,"");
        string expected=Get(body,"expected_symbol");
        if(expected!="" && !String.Equals(symbol,expected,StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("下單契約與即時行情契約不一致。");
        if(String.IsNullOrWhiteSpace(symbol) || (mode=="live" && sdk.GetProductInfo(symbol)==null && sdk.GetProductInfoT1(symbol)==null))
            throw new InvalidOperationException("券商商品檔未找到該契約。");
        var order=new OrderReference { Symbol=symbol, Side=side=="buy"?SIDE_FLAG.SF_BUY:SIDE_FLAG.SF_SELL };
        switch(Get(body,"time_in_force"))
        {
            case "rod": order.Duration=TIME_IN_FORCE.TIF_ROD; break;
            case "ioc": order.Duration=TIME_IN_FORCE.TIF_IOC; break;
            case "fok": order.Duration=TIME_IN_FORCE.TIF_FOK; break;
            default: throw new InvalidOperationException("請選擇 ROD、IOC 或 FOK。");
        }
        switch(Get(body,"position_effect"))
        {
            case "open": order.Effect=POSITION_EFFECT.PE_OPEN; break;
            case "close": order.Effect=POSITION_EFFECT.PE_CLOSE; break;
            case "day_trade": order.Effect=POSITION_EFFECT.PE_DAY_TRADE; break;
            default: throw new InvalidOperationException("請指定新倉、平倉或當沖。");
        }
        switch(Get(body,"price_type"))
        {
            case "limit": order.PriceFlag=PRICE_FLAG.PF_SPECIFIED; order.Price=Price(body); break;
            case "market":
                if(order.Duration==TIME_IN_FORCE.TIF_ROD) throw new InvalidOperationException("市價單請使用 IOC 或 FOK。");
                order.PriceFlag=PRICE_FLAG.PF_MARKET;
                break;
            default: throw new InvalidOperationException("請指定限價或市價。");
        }
        order.Quantity=Quantity(body);
        return order;
    }

    static decimal Price(Dictionary<string,object> body)
    {
        decimal number;
        if(!Decimal.TryParse(Get(body,"price"),NumberStyles.AllowDecimalPoint,Invariant,out number) || number<=0)
            throw new InvalidOperationException("請填入有效的正數限價。");
        return number;
    }
    static ushort Quantity(Dictionary<string,object> body)
    {
        ushort quantity;
        if(!UInt16.TryParse(Get(body,"quantity"),NumberStyles.None,Invariant,out quantity) || quantity==0)
            throw new InvalidOperationException("口數須為正整數。");
        return quantity;
    }

    void SendOrder(string id, string action, Dictionary<string,object> body)
    {
        OrderReference order;
        if(action=="place") order=NewOrder(body);
        else if(!working.TryGetValue(Get(body,"reference"),out order))
            throw new InvalidOperationException("此單未出現在目前連線的可操作委託清單。");
        decimal price=action=="modify_price"?Price(body):order.Price;
        ushort qty=action=="modify_quantity"?Quantity(body):order.Quantity;
        if(action=="modify_quantity" && qty>=order.Quantity)
            throw new InvalidOperationException("改量須小於目前回報口數。");
        if(mode=="paper")
        {
            if(action=="place")
            {
                order.Key="paper-"+(++paperCounter);
                order.Number=order.Key;
                working.Add(order.Key,order);
            }
            if(action=="cancel") working.Remove(order.Key);
            order.Price=price; order.Quantity=qty;
            Emit("order_report",new { id=id, reference=order.Key, symbol=order.Symbol, action=action, code=0,
                side=order.Side==SIDE_FLAG.SF_BUY?"B":"S", price=Text(price), quantity=action=="cancel"?0:qty,
                order_no=order.Number, simulated=true });
            return;
        }
        long request=sdk.GetRequestId();
        requests.Add(request,id);
        Emit("order_dispatch",new { id=id, request_id=Text(request) });
        long result;
        if(action=="place")
            result=sdk.Order(ORDER_TYPE.OT_NEW,MARKET_FLAG.MF_FUT,request,broker,account,trader,order.Symbol,order.Side,
                order.PriceFlag,price,order.Duration,qty,order.Effect,OFFICE_FLAG.OF_SPEEDY);
        else
        {
            ORDER_TYPE op=action=="cancel"?ORDER_TYPE.OT_CANCEL:action=="modify_price"?ORDER_TYPE.OT_MODIFY_PRICE:ORDER_TYPE.OT_MODIFY_QTY;
            result=sdk.Order(op,MARKET_FLAG.MF_FUT,request,broker,account,trader,order.Symbol,order.Side,
                order.PriceFlag,price,order.Duration,qty,order.Effect,OFFICE_FLAG.OF_SPEEDY,order.Web,order.Cnt,order.Number);
        }
        if(result==0) Reply(id,"submitted","SDK 已送出請求，等待凱基回報。");
        else Reply(id,"rejected","SDK 回傳碼 "+result);
    }

    void Stop(string reason)
    {
        ready=false;
        stop=true;
        Status("error",reason);
    }

    void Run()
    {
        Login();
        var input=new Thread(delegate() {
            string line;
            while((line=Console.ReadLine())!=null)
            {
                string request=line;
                queue.Enqueue(delegate { Handle(request); });
            }
            queue.Enqueue(delegate { stop=true; });
        });
        input.IsBackground=true;
        input.Start();
        try
        {
            while(!stop)
            {
                Application.DoEvents();
                Action action;
                for(int n=0; n<100 && !stop && queue.TryDequeue(out action);n++) action();
                if(mode!="paper" && !ready && DateTime.UtcNow>=loginDeadline)
                    Stop("交易登入或帳號核對逾時。");
                Thread.Sleep(10);
            }
        }
        finally
        {
            if(sdk!=null) { sdk.Logout(); sdk.Dispose(); }
        }
    }

    [STAThread]
    static int Main(string[] args)
    {
        Console.InputEncoding=new UTF8Encoding(false);
        Console.OutputEncoding=new UTF8Encoding(false);
        if(args.Length<1 || args.Length>3 || !new[]{"paper","read_only","live"}.Contains(args[0]) ||
            (args.Length>=2 && args[1]!="foreign") ||
            (args.Length==3 && (args[0]!="paper" || args[2]!="replay"))) return 2;
        var gateway=new TradeGateway(args[0],args.Length>=2,args.Length==3,Console.Out);
        Console.SetOut(TextWriter.Null);
        try { gateway.Run(); return 0; }
        catch(Exception error)
        {
            // Never print SDK messages that might include credentials or accounts.
            gateway.Status("error","交易轉接器中止（"+error.GetType().Name+"）。");
            return 1;
        }
    }
}
