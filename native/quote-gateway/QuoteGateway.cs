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
using Intelligence;
using Package;

internal static class QuoteGateway
{
    static readonly ManualResetEvent Connected = new ManualResetEvent(false);
    static readonly ManualResetEvent LoginReady = new ManualResetEvent(false);
    static readonly ManualResetEvent LoginPacket = new ManualResetEvent(false);
    static readonly ManualResetEvent QuoteTopicsReady = new ManualResetEvent(false);
    static readonly ManualResetEvent Subscribed = new ManualResetEvent(false);
    static readonly ManualResetEvent Stopped = new ManualResetEvent(false);
    static readonly BlockingCollection<Dictionary<string, object>> Events =
        new BlockingCollection<Dictionary<string, object>>(8192);
    static readonly object Gate = new object();
    static readonly string Epoch = Guid.NewGuid().ToString("N");
    static readonly JavaScriptSerializer Json = new JavaScriptSerializer();
    static readonly CultureInfo Invariant = CultureInfo.InvariantCulture;
    static Dictionary<string, string> instruments;
    static HashSet<string> pendingSubscriptions;
    static string connection;
    static bool rawPrice;
    static bool firstTrade;
    static readonly Dictionary<string, DateTime> ClockWarnings = new Dictionary<string, DateTime>();
    static readonly List<PendingTrade> PendingTrades = new List<PendingTrade>();
    static int decimals;
    static int tradeType;
    static bool nikkei;
    static string maturity;
    static string nikkeiSymbol;
    static string nikkeiContract;
    static readonly Dictionary<int, int> OverseasPackets = new Dictionary<int, int>();
    static DateTime lastOverseasReport = DateTime.MinValue;
    static int ignoredOverseasTrades;
    static readonly List<NearSubscription> NearSubscriptions = new List<NearSubscription>();
    sealed class NearSubscription
    {
        public string Product, Contract;
        public Dictionary<string, string> Expiries;
        public bool Confirmed;
        public DateTime ConfirmBy, RetryAfter;
    }
    static int failed;
    static long sequence;
    static bool accepting = true;

    sealed class PendingTrade
    {
        public DateTime Time;
        public long SourceSequence;
        public Dictionary<string, object> Fields;
    }

    static string Setting(string name)
    {
        string value = Environment.GetEnvironmentVariable(name);
        if (String.IsNullOrWhiteSpace(value))
            throw new InvalidOperationException("缺少設定 " + name);
        return value;
    }

    static DateTime TaipeiNow() { return DateTime.UtcNow.AddHours(8); }
    static DateTime MarketNow() { return nikkei ? DateTime.UtcNow.AddHours(9) : TaipeiNow(); }

    static string NearContract(string product, DateTime taipei, Dictionary<string, string> expiries)
    {
        var month = new DateTime(taipei.Year, taipei.Month, 1);
        var expiry = month.AddDays(14);
        while (expiry.DayOfWeek != DayOfWeek.Wednesday) expiry = expiry.AddDays(1);
        string date;
        if (expiries.TryGetValue(month.ToString("yyyyMM", Invariant), out date))
        {
            if (!DateTime.TryParseExact(date, "yyyy-MM-dd", Invariant, DateTimeStyles.None, out expiry) ||
                expiry.Year != month.Year || expiry.Month != month.Month)
                throw new InvalidOperationException("近月契約最後交易日設定無效。");
        }
        // 結算後先訂閱已掛牌的下一月，連續圖表於各商品夜盤開始時採用。
        if (taipei >= expiry.AddHours(13).AddMinutes(31)) month = month.AddMonths(1);
        return product + (char)('A' + month.Month - 1) + (month.Year % 10).ToString(Invariant);
    }

    static void RefreshNear(QuoteCom quote, DateTime now, NearSubscription near)
    {
        if (near.ConfirmBy != DateTime.MinValue)
        {
            lock (Gate) near.Confirmed = !pendingSubscriptions.Contains(near.Contract);
            if (!near.Confirmed && now < near.ConfirmBy) return;
            near.ConfirmBy = DateTime.MinValue;
            Emit("status", new Dictionary<string, object> { { "state", near.Product + (near.Confirmed ? "_ROLL_CONFIRMED" : "_ROLL_UNCONFIRMED") },
                { "provider_symbol", near.Contract } });
            if (!near.Confirmed)
            {
                quote.UnsubQuote(near.Contract, false);
                lock (Gate) { pendingSubscriptions.Remove(near.Contract); instruments.Remove(near.Contract); }
                near.RetryAfter = now.AddSeconds(30);
            }
            return;
        }
        if (now < near.RetryAfter) return;
        string next = NearContract(near.Product, now, near.Expiries);
        if (near.Confirmed && near.Contract == next) return;
        string old = near.Contract;
        lock (Gate)
        {
            if (old != next) instruments.Remove(old);
            near.Contract = next;
            instruments[next] = next;
            pendingSubscriptions.Add(next);
        }
        near.Confirmed = false;
        if (old != next) quote.UnsubQuote(old, false);
        short code = quote.SubQuote(next);
        Emit("status", new Dictionary<string, object> { { "state", near.Product + "_ROLL_SUBSCRIBE" }, { "provider_symbol", next }, { "code", code } });
        if (code == 0) near.ConfirmBy = now.AddSeconds(30);
        else
        {
            lock (Gate) { pendingSubscriptions.Remove(next); instruments.Remove(next); }
            near.RetryAfter = now.AddSeconds(30);
        }
    }
    static string Stamp(DateTime time) { return time.ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff", Invariant); }

    static void Fail(string reason, int code = 1)
    {
        if (Interlocked.CompareExchange(ref failed, code, 0) == 0)
            Console.Error.WriteLine(reason);
        Stopped.Set();
    }

    static void Emit(string kind, Dictionary<string, object> fields)
    {
        lock (Gate)
        {
            if (!accepting) return;
            fields["protocol_version"] = 1;
            fields["provider_id"] = "kgi_quotecom";
            fields["connection_id"] = connection;
            fields["connection_epoch"] = Epoch;
            fields["event_id"] = ++sequence;
            fields["received_at_utc"] = DateTime.UtcNow.ToString("o", Invariant);
            fields["type"] = kind;
            if (!Events.TryAdd(fields)) Fail("行情回呼佇列已滿，接收中止，請核對缺口。");
        }
    }

    static void WriteEvents()
    {
        try
        {
            foreach (var message in Events.GetConsumingEnumerable())
                Console.WriteLine(Json.Serialize(message));
        }
        catch (Exception) { Fail("行情管線寫入失敗。"); }
    }

    static void Status(object sender, COM_STATUS status, byte[] message)
    {
        try
        {
            var fields = new Dictionary<string, object> { { "state", status.ToString() }, { "sdk_code", (int)status } };
            if (status == COM_STATUS.SUBSCRIBE)
                ConfirmSubscription(message, fields);
            else
                Emit("status", fields);
            switch (status)
            {
                case COM_STATUS.CONNECT_READY: Connected.Set(); break;
                case COM_STATUS.LOGIN_READY: LoginReady.Set(); break;
                case COM_STATUS.CONNECT_FAIL:
                    Fail("QuoteCom 無法連線。", 2); break;
                case COM_STATUS.LOGIN_FAIL:
                case COM_STATUS.LOGIN_UNKNOW:
                    Fail("QuoteCom 回報 " + status + "，請核對連線與帳戶設定。"); break;
                case COM_STATUS.DISCONNECTED:
                    Fail("QuoteCom 報價連線中斷。", 2); break;
            }
        }
        finally { ((QuoteCom)sender).Processed(); }
    }

    static void ConfirmSubscription(byte[] message, Dictionary<string, object> fields)
    {
        lock (Gate)
        {
            var confirmed = new List<string>();
            string topic = message == null ? "" : Encoding.ASCII.GetString(message).TrimEnd('\0').Trim();
            if (topic == "TW.INFO") QuoteTopicsReady.Set();
            if (pendingSubscriptions != null && message != null)
            {
                foreach (string symbol in pendingSubscriptions)
                    if (Regex.IsMatch(topic, "(?<![A-Za-z0-9])" + Regex.Escape(symbol) + "(?![A-Za-z0-9])",
                                      RegexOptions.IgnoreCase | RegexOptions.CultureInvariant))
                        confirmed.Add(symbol);
                pendingSubscriptions.ExceptWith(confirmed);
            }
            fields["confirmed_symbols"] = confirmed;
            Emit("status", fields);
            if (pendingSubscriptions != null && pendingSubscriptions.Count == 0)
                Subscribed.Set();
        }
    }

    static DateTime TradeTime(long value)
    {
        long divisor = (long)Math.Pow(10, decimals);
        long clock = value / divisor;
        int hour = (int)(clock / 10000), minute = (int)(clock / 100 % 100), second = (int)(clock % 100);
        if (hour > 23 || minute > 59 || second > 59 || value < 0)
            throw new FormatException("成交時間原值 " + value.ToString(Invariant) + " 無法以 " + decimals + " 位小數解碼");
        var now = TaipeiNow();
        var result = now.Date.Add(new TimeSpan(hour, minute, second)).AddTicks((value % divisor) * (10000000 / divisor));
        if (result - now > TimeSpan.FromHours(12)) result = result.AddDays(-1);
        if (now - result > TimeSpan.FromHours(12)) result = result.AddDays(1);
        return result;
    }

    static void Trade(string symbol, long matchTime, decimal price, int scale, byte sign, decimal quantity,
                      decimal total, object sourceSequence, object lastItem, int type)
    {
        string instrument;
        if (!instruments.TryGetValue(symbol.Trim(), out instrument) || quantity <= 0) return;
        var time = TradeTime(matchTime);
        var now = TaipeiNow();
        if (time < now.AddMinutes(-2) || time > now.AddSeconds(5))
        {
            // 訂閱可能先送較早的成交快照。保留診斷，繼續接收即時成交。
            DateTime last;
            if (!ClockWarnings.TryGetValue(instrument, out last) || now - last >= TimeSpan.FromMinutes(1))
            {
                ClockWarnings[instrument] = now;
                Emit("status", new Dictionary<string, object> { {"state", "TRADE_OUTSIDE_CLOCK"},
                    {"instrument", instrument}, {"match_time_raw", matchTime},
                    {"decoded_time", Stamp(time)}, {"received_time", Stamp(now)} });
            }
            return;
        }
        if (rawPrice)
        {
            if (scale < 0 || scale > 6) throw new FormatException();
            for (int i = 0; i < scale; i++) price /= 10;
        }
        if (sign == (byte)'-') price = -Math.Abs(price);
        var fields = new Dictionary<string, object> {
            {"instrument",instrument}, {"provider_symbol",symbol}, {"event_time",Stamp(time)},
            {"price",price.ToString("0.############################",Invariant)},
            {"quantity",quantity.ToString(Invariant)}, {"cumulative_quantity",total.ToString(Invariant)},
            {"source_sequence",Convert.ToString(sourceSequence,Invariant)}, {"last_item",lastItem},
            {"sdk_type",type}, {"match_time_raw",matchTime}, {"price_sign_raw",sign}, {"price_decimal",scale}
        };
        QueueTrade(time, Convert.ToInt64(sourceSequence, Invariant), fields);
    }

    static void QueueTrade(DateTime time, long ordering, Dictionary<string, object> fields)
    {
        if (PendingTrades.Count >= 8192) { Fail("成交排序佇列已滿，請核對行情缺口。"); return; }
        PendingTrades.Add(new PendingTrade { Time = time, SourceSequence = ordering, Fields = fields });
    }

    static bool TryNikkeiTime(string raw, DateTime now, out DateTime time)
    {
        time = DateTime.MinValue;
        DateTime parsed;
        const DateTimeStyles style = DateTimeStyles.AllowWhiteSpaces | DateTimeStyles.NoCurrentDateDefault;
        string[] full = { "yyyyMMddHHmmss", "yyyyMMddHHmmssfff", "yyyyMMddHHmmss.FFFFFFF",
            "yyyy/MM/dd HH:mm:ss.FFFFFFF", "yyyy-MM-dd HH:mm:ss.FFFFFFF", "yyyy-MM-dd'T'HH:mm:ss.FFFFFFF" };
        string[] clock = { "HHmmss", "HHmmssff", "HHmmssfff", "HHmmss.FFFFFFF", "HH:mm:ss.FFFFFFF" };
        bool dated = DateTime.TryParseExact(raw, full, Invariant, style, out parsed);
        if (!dated && !DateTime.TryParseExact(raw, clock, Invariant, style, out parsed)) return false;
        int matches = 0;
        // 海外封包不含時區時，僅接受能和即時接收時鐘唯一對上的表示。
        foreach (int taipeiOrUtcToTokyo in new[] { 0, 1, 9 })
        {
            var candidate = (dated ? parsed : now.Date.Add(parsed.TimeOfDay)).AddHours(taipeiOrUtcToTokyo);
            if (!dated)
            {
                if (candidate - now > TimeSpan.FromHours(12)) candidate = candidate.AddDays(-1);
                if (now - candidate > TimeSpan.FromHours(12)) candidate = candidate.AddDays(1);
            }
            if (candidate < now.AddMinutes(-2) || candidate > now.AddSeconds(5)) continue;
            time = candidate;
            matches++;
        }
        return matches == 1;
    }

    static void NikkeiTrade(PI40020 p)
    {
        if (p.ComType != 'F' || !String.Equals(p.Symbol.Trim(), nikkeiSymbol, StringComparison.OrdinalIgnoreCase)
            || p.Quantity <= 0 || p.TotalQty <= 0)
        {
            if (ignoredOverseasTrades++ == 0)
                Emit("status", new Dictionary<string, object> { { "state", "JPX_MATCH_IGNORED" },
                    { "provider_symbol", p.Symbol.Trim() }, { "product_type", p.ComType.ToString() },
                    { "quantity", p.Quantity }, { "cumulative_quantity", p.TotalQty } });
            return;
        }
        if (p.Price <= 0 || p.Price % 5 != 0) { Fail("JPX 225M 成交價格未符合 5 點跳動單位。"); return; }
        var now = MarketNow();
        DateTime time;
        if (!TryNikkeiTime(p.TradeTime, now, out time))
        {
            DateTime last;
            if (!ClockWarnings.TryGetValue(nikkeiContract, out last) || now - last >= TimeSpan.FromMinutes(1))
            {
                ClockWarnings[nikkeiContract] = now;
                Emit("status", new Dictionary<string, object> { {"state", "JPX_TRADE_TIME_UNVERIFIED"},
                    {"instrument", nikkeiContract}, {"trade_time_raw", p.TradeTime}, {"received_time", Stamp(now)} });
            }
            return;
        }
        QueueTrade(time, p.TotalQty, new Dictionary<string, object> {
            {"instrument",nikkeiContract}, {"provider_symbol",p.Symbol}, {"exchange","JPX"}, {"maturity",maturity},
            {"event_time",Stamp(time)}, {"price",p.Price.ToString(Invariant)},
            {"quantity",p.Quantity.ToString(Invariant)}, {"cumulative_quantity",p.TotalQty},
            {"foreign_trade_key",Stamp(time) + ":" + p.Session + ":" + p.TotalQty.ToString(Invariant)},
            {"session",p.Session}, {"trade_time_raw",p.TradeTime}, {"sdk_type",p.DT}
        });
    }

    // 以成交鐘與來源序號整理短暫亂序，收棒前先釋出達界線的成交。
    static void FlushTrades(DateTime cutoff)
    {
        var due = PendingTrades.Where(trade => trade.Time <= cutoff)
            .OrderBy(trade => trade.Time).ThenBy(trade => trade.SourceSequence).ToArray();
        PendingTrades.RemoveAll(trade => trade.Time <= cutoff);
        foreach (var trade in due) Emit("trade", trade.Fields);
        firstTrade |= due.Length > 0;
    }

    static void Receive(object sender, PackageBase package)
    {
        lock (Gate)
        {
            try
            {
                if (nikkei && package.DT >= 40000 && package.DT < 40100)
                {
                    int count;
                    OverseasPackets.TryGetValue(package.DT, out count);
                    OverseasPackets[package.DT] = count + 1;
                }
                if (package.DT == (ushort)DT.LOGIN)
                {
                    var login = (P001503)package;
                    if (login.Code != 0) { Fail("QuoteCom 登入錯誤碼 " + login.Code); return; }
                    Emit("status", new Dictionary<string, object> { { "state", "LOGIN_ACCEPTED" }, { "login_code", 0 } });
                    if ((nikkei ? 1 : instruments.Count) > Convert.ToInt32(login.Qnum)) { Fail("商品數超過凱基核發額度。"); return; }
                    LoginPacket.Set();
                }
                else if (nikkei && package.DT == (ushort)DT.QUOTE_FOREIGN_MATCH)
                    NikkeiTrade((PI40020)package);
                else if (!nikkei && package.DT == tradeType && tradeType == (ushort)DT.QUOTE_I020)
                {
                    var p = (PI20020)package;
                    Trade(p.Symbol, Convert.ToInt64(p.MatchTime), Convert.ToDecimal(p.Price), Convert.ToInt32(p.PriceDecimal),
                          p.PriceSign, Convert.ToDecimal(p.MatchQuantity), Convert.ToDecimal(p.MatchTotalQty), p.InfoSeq, p.LastItem, p.DT);
                }
                else if (!nikkei && package.DT == tradeType && tradeType == (ushort)DT.QUOTE_I024)
                {
                    var p = (PI20024)package;
                    if (p.Session != '0' && p.Session != '\0') return;
                    Trade(p.Symbol, Convert.ToInt64(p.MatchTime), Convert.ToDecimal(p.Price), Convert.ToInt32(p.PriceDecimal),
                          p.PriceSign, Convert.ToDecimal(p.MatchQuantity), Convert.ToDecimal(p.MatchTotalQty), p.InfoSeq, p.LastItem, p.DT);
                }
            }
            catch (FormatException error) { Fail("QuoteCom 封包格式無效：" + error.Message); }
            catch (Exception error) { Fail("QuoteCom 封包解析失敗（" + error.GetType().Name + "），請核對時間及價格格式。"); }
        }
    }

    static void WaitFor(WaitHandle ready, string step)
    {
        int signal = WaitHandle.WaitAny(new WaitHandle[] { Stopped, ready }, TimeSpan.FromSeconds(30));
        if (signal == WaitHandle.WaitTimeout) Fail(step + "未獲確認。", 2);
        if (signal != 1) throw new InvalidOperationException(step + "未獲確認。");
    }

    static void Run(string[] args)
    {
        nikkei = args.Length > 0 && args[0] == "jpx225m";
        if (nikkei)
        {
            DateTime month;
            if (args.Length != 2 || !DateTime.TryParseExact(args[1], "yyyyMM", Invariant, DateTimeStyles.None, out month))
                throw new InvalidOperationException("用法：QuoteGateway jpx225m YYYYMM");
            maturity = args[1];
            nikkeiContract = "225M-" + maturity;
            connection = Setting("QUOTECOM_CONNECTION") + ":jpx225m:" + maturity;
        }
        else
        {
        if (args.Length < 4)
            throw new InvalidOperationException("用法：QuoteGateway 020|024 時間小數位 raw|decoded 券商全碼=內部契約 [其他對照…]");
        tradeType = args[0] == "020" ? 20020 : args[0] == "024" ? 20024 : 0;
        if (tradeType == 0 || !Int32.TryParse(args[1], out decimals) || decimals < 0 || decimals > 6 ||
            (args[2] != "raw" && args[2] != "decoded"))
            throw new InvalidOperationException("請指定成交封包、時間小數位（0～6）及價格編碼。");
        rawPrice = args[2] == "raw";
        instruments = args.Skip(3).Where(value => !value.StartsWith("--auto-", StringComparison.Ordinal)).Select(value => value.Split('=')).ToDictionary(
            pair => pair[0], pair => pair.Length == 2 && pair[0].Length > 0 && pair[1].Length > 0
                ? pair[1] : throwMapping(), StringComparer.OrdinalIgnoreCase);
        foreach (string option in args.Skip(3).Where(value => value.StartsWith("--auto-", StringComparison.Ordinal)))
        {
            var parts = option.Split(new[] { '=' }, 2);
            string product = parts[0].Substring("--auto-".Length).ToUpperInvariant();
            if (parts.Length != 2 || (product != "TXF" && product != "MXF" && product != "TMF" && product != "CDF") || NearSubscriptions.Any(n => n.Product == product) ||
                instruments.Keys.Any(k => Regex.IsMatch(k, "^" + product + "[A-L][0-9]$", RegexOptions.IgnoreCase)))
                throw new InvalidOperationException("自動近月模式請勿重複填寫同商品的固定月份契約。");
            var expiries = Json.Deserialize<Dictionary<string, string>>(File.ReadAllText(parts[1]));
            if (expiries == null) throw new InvalidOperationException("缺少最後交易日設定。");
            var contract = NearContract(product, TaipeiNow(), expiries);
            instruments.Add(contract, contract);
            NearSubscriptions.Add(new NearSubscription { Product = product, Contract = contract, Expiries = expiries });
        }
        if (instruments.Values.Distinct(StringComparer.OrdinalIgnoreCase).Count() != instruments.Count)
            throw new InvalidOperationException("每個券商全碼須對應不同的實際契約。");
        connection = String.Join(":", Setting("QUOTECOM_CONNECTION"), args[0], args[1], args[2]);
        }
        string host = Setting("QUOTECOM_HOST");
        ushort port = UInt16.Parse(Setting("QUOTECOM_PORT"), Invariant);
        using (var quote = new QuoteCom(host, port, Setting("QUOTECOM_SOURCE_ID"), Setting("QUOTECOM_TOKEN")))
        {
            quote.OnGetStatus += Status;
            quote.OnRcvMessage += Receive;
            quote.Connect(host, port);
            WaitFor(Connected, "連線");
            quote.Login2(Setting("QUOTECOM_ID"), Setting("QUOTECOM_PASSWORD"));
            WaitFor(LoginReady, "登入狀態");
            WaitFor(LoginPacket, "帳號回報");
            short code;
            if (nikkei)
            {
                var exchange = MARKET_EXCHANGE.ME_JPX;
                var future = MARKET_FLAG.MF_FUT;
                // 外期訂閱須等報價主題就緒。JPX 商品清單使用 225M 作為小型日經報價簡碼。
                WaitFor(QuoteTopicsReady, "行情主題 TW.INFO");
                nikkeiSymbol = quote.GenForeignFullComID(exchange, future, "225M", maturity, "", "", "", "");
                if (String.IsNullOrWhiteSpace(nikkeiSymbol)) throw new InvalidOperationException("凱基 SDK 無法產生大阪日經 mini 契約全碼。");
                lock (Gate) pendingSubscriptions = new HashSet<string>(new[] { nikkeiSymbol }, StringComparer.OrdinalIgnoreCase);
                code = quote.SubQuote(exchange, future, nikkeiSymbol);
                Emit("status", new Dictionary<string, object> { {"state","JPX_SUBSCRIPTION_REQUEST"},
                    {"instrument",nikkeiContract}, {"provider_symbol",nikkeiSymbol}, {"code",code} });
                if (code != 0) throw new InvalidOperationException("大阪日經 mini 訂閱錯誤碼 " + code + "：" + quote.GetSubQuoteMsg(code));
            }
            else
            {
                lock (Gate) pendingSubscriptions = new HashSet<string>(instruments.Keys, StringComparer.OrdinalIgnoreCase);
                code = instruments.Count == 1 ? quote.SubQuote(instruments.Keys.First()) : quote.SubQuotes(String.Join("|", instruments.Keys));
                if (code != 0) throw new InvalidOperationException("訂閱錯誤碼 " + code);
            }
            WaitFor(Subscribed, "訂閱");
            if (nikkei)
            {
                short snapshotCode = quote.RetriveLastPrice(MARKET_EXCHANGE.ME_JPX, MARKET_FLAG.MF_FUT,
                    FOREIGN_SESSION.FS_EC, nikkeiSymbol);
                Emit("status", new Dictionary<string, object> { { "state", "JPX_SNAPSHOT_REQUEST" },
                    { "provider_symbol", nikkeiSymbol }, { "code", snapshotCode } });
            }
            foreach (var near in NearSubscriptions)
            {
                near.Confirmed = true;
                Emit("status", new Dictionary<string, object> { { "state", near.Product + "_NEAR_READY" }, { "provider_symbol", near.Contract } });
            }
            Emit("status", new Dictionary<string, object> { { "state", "QUOTE_READY" } });
            DateTime lastWatermark = DateTime.MinValue;
            while (!Stopped.WaitOne(250))
            {
                lock (Gate)
                {
                    DateTime receivedTime = MarketNow();
                    FlushTrades(receivedTime.AddSeconds(-2));
                    if (nikkei && !firstTrade && receivedTime >= lastOverseasReport.AddMinutes(1))
                    {
                        lastOverseasReport = receivedTime;
                        Emit("status", new Dictionary<string, object> { { "state", "JPX_WAITING_FOR_TRADE" },
                            { "instrument", nikkeiContract }, { "checked_at", Stamp(receivedTime) },
                            { "received_packets", OverseasPackets.ToDictionary(p => p.Key.ToString(Invariant), p => p.Value) },
                            { "ignored_matches", ignoredOverseasTrades },
                            { "exchanges", (quote.List_Exchange ?? new List<string>()).ToArray() } });
                    }
                    DateTime now = receivedTime.AddSeconds(-10);
                    DateTime cutoff = now.AddTicks(-(now.Ticks % TimeSpan.TicksPerMinute));
                    if (firstTrade && cutoff > lastWatermark)
                    {
                        Emit("watermark", new Dictionary<string, object> { {"event_time",Stamp(cutoff)} });
                        lastWatermark = cutoff;
                    }
                }
                foreach (var near in NearSubscriptions) RefreshNear(quote, MarketNow(), near);
            }
            quote.OnGetStatus -= Status;
            quote.OnRcvMessage -= Receive;
            quote.Logout();
        }
    }

    static string throwMapping() { throw new InvalidOperationException("商品對照必須為 券商全碼=內部契約"); }

    [STAThread]
    static int Main(string[] args)
    {
        Console.OutputEncoding = new UTF8Encoding(false);
        Console.CancelKeyPress += delegate(object sender, ConsoleCancelEventArgs e) { e.Cancel = true; Stopped.Set(); };
        var output = new Thread(WriteEvents) { IsBackground = true };
        output.Start();
        if (Environment.GetEnvironmentVariable("QUOTECOM_SUPERVISED") == "1")
        {
            var control = new Thread(delegate() { Console.ReadLine(); Stopped.Set(); }) { IsBackground = true };
            control.Start();
        }
        try { Run(args); }
        catch (InvalidOperationException e) { if (!Stopped.WaitOne(0)) Fail(e.Message); }
        catch (Exception e) { Fail("QuoteCom 啟動失敗（" + e.GetType().Name + "）。"); }
        finally
        {
            lock (Gate) { FlushTrades(DateTime.MaxValue); accepting = false; Events.CompleteAdding(); }
            output.Join();
        }
        return failed;
    }
}
