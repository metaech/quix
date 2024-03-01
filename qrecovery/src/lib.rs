// pub mod data_space;
pub mod crypto;
pub mod frame_queue;
pub mod index_deque;
pub mod recv;
pub mod rtt;
pub mod rx;
pub mod send;
pub mod space;
pub mod streams;
pub mod tx;

#[derive(Debug)]
pub enum AppStream {
    ReadOnly(recv::Reader),
    WriteOnly(send::Writer),
    ReadWrite(recv::Reader, send::Writer),
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
