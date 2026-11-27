//! 逐列解碼行情CSV，不修改原檔；雜湊仍由呼叫端對原始位元組計算。
use std::{fs::File,io::{BufRead,BufReader,Read},path::Path};

#[derive(Clone,Copy)]
enum Encoding {Utf8,Big5}

#[cfg(windows)]
pub(crate) fn big5(bytes:&[u8])->Result<String,String> {
    #[link(name="kernel32")]
    extern "system" {fn MultiByteToWideChar(page:u32,flags:u32,src:*const u8,len:i32,dst:*mut u16,capacity:i32)->i32;}
    if bytes.is_empty(){return Ok(String::new());}
    let len=i32::try_from(bytes.len()).map_err(|_|"文字欄位過長")?;
    unsafe {
        let n=MultiByteToWideChar(950,8,bytes.as_ptr(),len,std::ptr::null_mut(),0);
        if n==0{return Err("檔案不是有效的UTF-8或Big5／CP950文字，請另存UTF-8 CSV後再匯入".into());}
        let mut wide=vec![0u16;n as usize];
        if MultiByteToWideChar(950,8,bytes.as_ptr(),len,wide.as_mut_ptr(),n)!=n{return Err("Big5／CP950文字解碼失敗，未匯入資料".into());}
        String::from_utf16(&wide).map_err(|_|"Big5文字轉換失敗，未匯入資料".into())
    }
}

#[cfg(all(test,windows))]
mod tests {
    use super::*;
    use crate::{canonical_market,database};
    use sha2::{Digest,Sha256};
    use std::fs;
    const HEADER:&[u8]=b"\xa4\xe9\xb4\xc1,\xb6\x7d\xbd\x4c\xbb\xf9,\xb3\xcc\xb0\xaa\xbb\xf9,\xb3\xcc\xa7\x43\xbb\xf9,\xa6\xac\xbd\x4c\xbb\xf9,\xa6\xa8\xa5\xe6\xb6\x71,";
    #[test]
    fn ohlcv_is_not_tick_and_does_not_overwrite_existing_trades(){
        let dir=crate::workbench::root().join("outputs").join(format!("format-guard-test-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir_all(&dir).unwrap();
        let db=dir.join("market.sqlite");let file=dir.join("source.csv");
        fs::write(&file,"DateTime,Price,Volume\n2026/10/19 17:26:00,100,1\n").unwrap();
        let tick=canonical_market::import(&db,&file,"FORMAT_TEST","tick","Asia/Taipei","event").unwrap();
        fs::write(&file,"DateTime,Open,High,Low,Close,Volume\n20261019 17:26,2420,2420,2417,2417,35\n").unwrap();
        assert_eq!(inspect(&file).unwrap()["shape"],"bar");
        let error=canonical_market::import(&db,&file,"FORMAT_TEST","tick","Asia/Taipei","event").unwrap_err();
        assert!(error.contains("不是Tick"));
        let c=database::open_existing(&db).unwrap();let count:i64=c.query_row("SELECT row_count FROM datasets WHERE id=?1",[tick["dataset_id"].as_i64().unwrap()],|r|r.get(0)).unwrap();assert_eq!(count,1);
        let bar=canonical_market::import(&db,&file,"FORMAT_TEST","minute","Asia/Taipei","end").unwrap();assert_eq!(bar["input_rows"],1);
    }
    #[test]
    fn big5_ohlcv_title_dates_raw_hash_and_failed_replacement_are_preserved(){
        let dir=crate::workbench::root().join("outputs").join(format!("encoding-test-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();
        let file=dir.join("big5.csv");let db=dir.join("market.sqlite");
        let mut raw=HEADER.to_vec();raw.extend_from_slice(b"\r\n20261019 17:26,2420.00,2420.00,2417.00,2417.00,35,\r\n");
        fs::write(&file,&raw).unwrap();
        canonical_market::initialize(&crate::workbench::root(),&db).unwrap();
        let receipt=canonical_market::import(&db,&file,"BIG5_TEST","minute","Asia/Taipei","end").unwrap();
        let c=database::open_existing(&db).unwrap();
        let (time,payload,hash):(String,String,String)=c.query_row("SELECT r.datetime,r.payload,d.sha256 FROM records r JOIN datasets d ON d.id=r.dataset_id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(time,"2026-10-19T17:26:00.000000000");
        assert!(payload.contains("2420.00"));assert!(payload.contains("2417.00"));
        assert_eq!(receipt["source_sha256"],format!("{:x}",Sha256::digest(&raw)));
        drop(c);
        let mut title="瀏覽歷史資料 - 獨立驗證\r\n".as_bytes().to_vec();
        title.extend_from_slice("日期,開盤價,最高價,最低價,收盤價,成交量,\r\n20261019 17:26,99,100,98,99,1,\r\n20261019 17:27,1,0,2,1,1,\r\n".as_bytes());
        fs::write(&file,title).unwrap();
        assert!(canonical_market::import(&db,&file,"BIG5_TEST","minute","Asia/Taipei","end").is_err());
        let c=database::open_existing(&db).unwrap();
        assert_eq!(c.query_row("SELECT payload FROM records LIMIT 1",[],|r|r.get::<_,String>(0)).unwrap(),payload);
        assert_eq!(c.query_row("SELECT sha256 FROM datasets LIMIT 1",[],|r|r.get::<_,String>(0)).unwrap(),hash);
        assert!(receipt.is_object());
        let bad=b"\xa4\xe9\xb4\xc1,\xa6\xac\xbd\x4c\xbb\xf9,\xa6\xa8\xa5\xe6\xb6\x71\r\n20261019 17:26,2417,35\r\n";
        fs::write(&file,bad).unwrap();
        let error=canonical_market::import(&db,&file,"BIG5_TEST","minute","Asia/Taipei","end").unwrap_err();
        assert!(error.contains("open")&&error.contains("high")&&error.contains("low"));
    }
    #[test]
    fn csv_title_and_trailing_commas_do_not_hide_data_or_bad_encoding(){
        let text="瀏覽歷史資料 - 測試\n日期,開盤價,最高價,最低價,收盤價,成交量,\n20261019 17:26,1,2,0,1,3,\n";
        let mut r=TextCsv::new(text.as_bytes(),b',').unwrap();
        assert_eq!(r.headers().unwrap().len(),6);
        assert_eq!(r.records().next().unwrap().unwrap().len(),6);
        assert!(big5(&[0xa4]).is_err());
        let data=b"Date,Time,Price,Volume\n2026/10/19,17:26:00,1,1,EXTRA\n";
        assert!(TextCsv::new(&data[..],b',').unwrap().records().next().unwrap().is_err());
    }
}
#[cfg(not(windows))]
fn big5(_: &[u8])->Result<String,String>{Err("此平台請先將Big5／CP950行情另存為UTF-8 CSV再匯入".into())}

fn decode(row:&csv::ByteRecord,encoding:Encoding)->Result<csv::StringRecord,String> {
    if matches!(encoding,Encoding::Utf8) || row.as_slice().is_ascii(){
        let mut record=csv::StringRecord::from_byte_record(row.clone()).map_err(|_|"行情含非UTF-8文字；請確認全檔使用同一編碼，或另存UTF-8 CSV")?;
        while !record.is_empty() && record.get(record.len()-1).is_some_and(|v|v.trim().is_empty()){record.truncate(record.len()-1);}
        return Ok(record);
    }
    let mut values=Vec::new();
    for bytes in row.iter(){
        values.push(match encoding {
            Encoding::Utf8=>std::str::from_utf8(bytes).map_err(|_|"行情含非UTF-8文字；請確認全檔使用同一編碼，或另存UTF-8 CSV")?.to_owned(),
            Encoding::Big5=>big5(bytes)?,
        });
    }
    while values.last().is_some_and(|v|v.trim().is_empty()){values.pop();}
    Ok(csv::StringRecord::from(values))
}

pub fn delimiter(path:&Path)->Result<u8,String>{
    let mut bytes=Vec::new();
    let mut reader=BufReader::new(File::open(path).map_err(|e|e.to_string())?).take(1024*1024);
    reader.read_until(b'\n',&mut bytes).map_err(|e|e.to_string())?;
    reader.read_until(b'\n',&mut bytes).map_err(|e|e.to_string())?;
    if bytes.starts_with(&[0xff,0xfe]) || bytes.starts_with(&[0xfe,0xff]) {return Err("目前不接受UTF-16行情檔，請另存UTF-8或Big5 CSV再匯入".into());}
    // 標題列可以沒有分隔符；檢查前兩行，檔案其餘部分仍串流讀取。
    let prefix=bytes.split(|b|*b==b'\n').take(2).flatten().copied().collect::<Vec<_>>();
    Ok(if prefix.contains(&b'|'){b'|'}else if prefix.contains(&b'\t'){b'\t'}else{b','})
}

pub struct TextCsv<R:Read>{reader:csv::Reader<R>,header:csv::StringRecord,encoding:Encoding}
pub fn inspect(path:&Path)->Result<serde_json::Value,String>{
    let mut reader=TextCsv::new(File::open(path).map_err(|e|e.to_string())?,delimiter(path)?)?;
    let columns=reader.headers()?.iter().map(crate::database::normalized).collect::<Vec<_>>();
    let has=|name:&str|columns.iter().any(|x|x==name);
    let shape=if has("price")&&has("volume"){"tick"}else if ["open","high","low","close","volume"].iter().all(|x|has(x)){"bar"}else{
        if has("close")&&!has("price") {
            let missing=["open","high","low","volume"].iter().filter(|x|!has(x)).copied().collect::<Vec<_>>();
            return Err(format!("K棒缺少欄位：{}；不能只用收盤價補造OHLC，也不會將Close當成逐筆price。",missing.join("、")));
        }
        return Err(format!("無法辨識行情格式。Tick須有成交價 price 與成交量；K棒須有 Open、High、Low、Close、Volume。檔案欄位：{}；不會將Close當成逐筆成交價。",columns.join("、")));
    };
    let first=reader.records().next().ok_or("匯入檔沒有資料")??;
    let sample=columns.iter().position(|x|x=="datetime"||x=="date").and_then(|i|first.get(i)).unwrap_or("");
    let intraday=sample.contains(':')||has("time");
    Ok(serde_json::json!({"shape":shape,"columns":columns,"suggested_kind":if shape=="tick"{"tick"}else if intraday{"minute"}else{"daily"},"period_confirm_required":shape=="bar"}))
}
pub fn check_kind(path:&Path,kind:&str)->Result<serde_json::Value,String>{
    let info=inspect(path)?;
    if kind=="tick"&&info["shape"]=="bar" {return Err("檔案是OHLCV K棒資料，不是Tick。請選擇正確的「1分K」或「日K」格式；不會把Close改作price。尚未匯入任何行情。".into());}
    if kind!="tick"&&info["shape"]=="tick" {return Err("檔案是Tick成交資料，請將格式選為Tick；若需K棒，匯入Tick後再執行補建。尚未匯入任何行情。".into());}
    Ok(info)
}
impl<R:Read> TextCsv<R>{
    pub fn new(source:R,delimiter:u8)->Result<Self,String>{
        let mut reader=csv::ReaderBuilder::new().delimiter(delimiter).trim(csv::Trim::All).has_headers(false).flexible(true).from_reader(source);
        let mut raw=csv::ByteRecord::new();
        if !reader.read_byte_record(&mut raw).map_err(|e|e.to_string())?{return Err("資料檔為空".into());}
        let encoding=if std::str::from_utf8(raw.as_slice()).is_ok(){Encoding::Utf8}else{Encoding::Big5};
        let mut header=decode(&raw,encoding)?;
        if header.len()==1 && header[0].trim_start_matches('\u{feff}').starts_with("瀏覽歷史資料 -"){
            if !reader.read_byte_record(&mut raw).map_err(|e|e.to_string())?{return Err("匯出檔只有標題，沒有資料欄位".into());}
            header=decode(&raw,encoding)?;
        }
        Ok(Self{reader,header,encoding})
    }
    pub fn headers(&self)->Result<&csv::StringRecord,String>{Ok(&self.header)}
    pub fn records(&mut self)->impl Iterator<Item=Result<csv::StringRecord,String>>+'_ {
        let encoding=self.encoding;let width=self.header.len();
        self.reader.byte_records().map(move|r|{
            let raw=r.map_err(|e|e.to_string())?;
            let row=decode(&raw,encoding)?;
            if row.len()!=width{return Err(format!("CSV欄數不符：應有{width}欄，實際{}欄，未補造缺欄資料",row.len()));}
            Ok(row)
        })
    }
    pub fn into_inner(self)->R{self.reader.into_inner()}
}
