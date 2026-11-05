//! Schlanke Hülle um die Windows-Leistungsindikatoren (PDH).

use windows::core::HSTRING;
use windows::Win32::System::Performance::*;

const PDH_MORE_DATA: u32 = 0x800007D2;
const PDH_FMT_NOCAP100_: u32 = 0x8000;

pub struct Query {
    h: isize,
}

#[derive(Clone, Copy)]
pub struct Counter(isize);

impl Query {
    pub fn new() -> Option<Query> {
        let mut h: isize = 0;
        let st = unsafe { PdhOpenQueryW(None, 0, &mut h) };
        (st == 0).then_some(Query { h })
    }

    /// Fügt einen Zähler über seinen englischen Pfad hinzu (unabhängig von der Systemsprache).
    pub fn add(&self, path: &str) -> Option<Counter> {
        let mut c: isize = 0;
        let st = unsafe { PdhAddEnglishCounterW(self.h, &HSTRING::from(path), 0, &mut c) };
        (st == 0).then_some(Counter(c))
    }

    pub fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.h) == 0 }
    }

    pub fn value(&self, c: Counter) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let st = unsafe { PdhGetFormattedCounterValue(c.0, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100_), None, &mut v) };
        if st != 0 || (v.CStatus != 0 && v.CStatus != 1) {
            return None;
        }
        let d = unsafe { v.Anonymous.doubleValue };
        d.is_finite().then_some(d)
    }

    /// Alle Instanzen eines Platzhalter-Zählers: (Instanzname, Wert).
    pub fn array(&self, c: Counter) -> Vec<(String, f64)> {
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100_);
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let st = unsafe { PdhGetFormattedCounterArrayW(c.0, fmt, &mut size, &mut count, None) };
        if st != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        // Puffer in u64 ausrichten
        let mut buf: Vec<u64> = vec![0; (size as usize + 7) / 8 + 1];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        let st = unsafe { PdhGetFormattedCounterArrayW(c.0, fmt, &mut size, &mut count, Some(items)) };
        if st != 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let item = unsafe { &*items.add(i) };
            if item.FmtValue.CStatus != 0 && item.FmtValue.CStatus != 1 {
                continue;
            }
            let name = unsafe { item.szName.to_string() }.unwrap_or_default();
            let v = unsafe { item.FmtValue.Anonymous.doubleValue };
            if v.is_finite() {
                out.push((name, v));
            }
        }
        out
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.h);
        }
    }
}
