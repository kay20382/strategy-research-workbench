//! 面向使用者的日期格式；資料庫排序鍵、原始證據與奈秒識別不改寫。
pub fn display(value:&str)->String {
    let s=value.trim();
    if s.len()>=10 && s.as_bytes().get(4).is_some_and(|b|*b==b'-'||*b==b'/') {
        let date=s[..10].replace('-',"/");
        if s.len()>=19 {return format!("{} {}",date,&s[11..19]);}
        return date;
    }
    s.into()
}
#[cfg(test)]
mod tests{
    #[test]fn display_never_changes_the_source_precision(){
        let raw="2026-10-23T02:34:00.123456789";
        assert_eq!(super::display(raw),"2026/10/23 02:34:00");
        assert_eq!(raw,"2026-10-23T02:34:00.123456789");
        assert_eq!(super::display("2026-10-23"),"2026/10/23");
    }
}
