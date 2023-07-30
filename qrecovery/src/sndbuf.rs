use bytes::BufMut;
use crossbeam_skiplist::{map::Entry, SkipMap};
use slice_deque::SliceDeque;
use std::ops::{Bound::Included, Range};

// 标识一段数据的状态，既染色
#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum Color {
    Pending,
    Fligting,
    Recved,
    Lost,
}

pub struct SendBuf {
    offset: u64,
    // 通过MAX_DATA_FRAME帧及时告知，不会超过2^62
    max_data_len: u64,
    // 写入数据的环形队列，与接收队列不同的是，它是连续的
    data: SliceDeque<u8>,
    // 这是一个无锁的高效有序跳表
    // 它的意义是，从前一段(如果是第一个，则是offset)到key的range范围的数据，是value这个Color的
    state: SkipMap<u64, Color>,
}

impl SendBuf {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            offset: 0,
            max_data_len: n as u64,
            data: SliceDeque::with_capacity(n),
            state: SkipMap::new(),
        }
    }

    pub fn writeable(&self) -> bool {
        self.max_data_len > self.wrote()
    }

    // invoked by application layer
    pub fn write(&mut self, data: &[u8]) -> usize {
        assert!(!data.is_empty());

        // 写的数据量受流量控制限制
        let wrote = self.wrote();
        let writeable = self.max_data_len - wrote;
        let n = std::cmp::min(writeable as usize, data.len());
        if n > 0 {
            self.data.copy_from_slice(&data[..n]);

            let wrote = wrote + n as u64;
            let entry = self.state.insert(wrote, Color::Pending);
            // 前向Pending合并，有助于提高效率，包大块发送
            let mut prev = entry.prev();
            loop {
                match prev {
                    Some(e) if *e.value() == Color::Pending => {
                        prev = e.prev();
                        e.remove();
                    }
                    _ => break,
                }
            }
        }

        n
    }

    // invoked by application layer
    // when being ended gracefully, the result means the final size.
    pub fn wrote(&self) -> u64 {
        match self.state.back() {
            None => self.offset,
            Some(e) => *e.key(),
        }
    }

    // 无需close：不在写入即可，具体到某个状态，才有close
    // 无需reset：状态转化间，需要reset，而Sender上下文直接释放即可
    // 无需clean：Sender上下文直接释放即可，

    // feedback from transport layer
    // ack只能确认F/L状态的区间
    pub fn ack(&mut self, range: Range<u64>) {
        let mut pre_color = None;
        for item in self.state.range(range.clone()) {
            if pre_color.is_none() {
                pre_color = Some(*item.value());
            }

            match *item.value() {
                Color::Fligting | Color::Lost | Color::Recved => {
                    item.remove();
                }
                // THINK: 是忽略，还是报错？直接崩溃肯定是不行的！
                Color::Pending => unreachable!("must not be acked before sending"),
            }
        }

        // 如果pre color是None，代表着range范围是一个小范围
        let color = match pre_color {
            Some(c) => c,
            None => *self
                .state
                .lower_bound(Included(&range.end))
                .unwrap()
                .value(),
        };
        let mut entry = Some(self.state.insert(range.start, color));
        // 前向Recved颜色合并
        loop {
            match entry {
                Some(e) if *e.value() == Color::Recved => {
                    entry = e.prev();
                    e.remove();
                }
                _ => break,
            }
        }

        let mut entry = self.state.insert(range.end, Color::Recved);
        // 后向Recved颜色的合并
        loop {
            let next = entry.next();
            match next {
                Some(e) if *e.value() == Color::Recved => {
                    entry.remove();
                    entry = e;
                }
                _ => break,
            }
        }
    }

    // invoked by transport layer
    pub fn emit(&self, n: usize) -> Option<&[u8]> {
        todo!("collect data to really send")
    }
}

#[cfg(test)]
mod test {

    #[test]
    fn it_works() {
        println!("hello");
    }
}
