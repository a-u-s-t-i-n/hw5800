use std::collections::VecDeque;

use log::info;

use crc16::CrcType;
//use textplots::{Chart, Plot, Shape};

fn crc(b: &[u8]) -> u16 {
    let v = crc16::BUYPASS::init();
    let u = crc16::BUYPASS::update(v, &b);
    crc16::BUYPASS::get(u)
}

fn ary_to_hex(msg: &[u8]) -> String {
    let v: Vec<String> = msg.iter().map(|b| format!("{:02X}", b)).collect();
    v.join(" ")
}

#[derive(Debug, Copy, Clone)]
pub struct HW5800Status {
    id: u32, // actually 3 bytes
    bits: u8,
}

impl HW5800Status {
    /// Construct a new HW5800Status object.
    /// Expects at least 4 bytes in the provided
    /// buffer, where the first 3 bytes are the
    /// little endian encoded devide ID and the
    /// 4th byte contains the status bits.
    ///
    /// # Panics
    /// Panics if at least 4 bytes aren't provided.
    pub fn new(m: &[u8]) -> Self {
        assert!(m.len() > 3);
        HW5800Status {
            id: ((m[0] as u32) << 16) + ((m[1] as u32) << 8) + m[2] as u32,
            bits: m[3],
        }
    }

    /// Return the devide ID for this status message
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the bitfield status from this status message.
    /// These have different meanings depending on the type
    /// of the device.
    pub fn bits(&self) -> u8 {
        self.bits
    }
}

// See the README for a description of the processing algorithm.

#[derive(Debug, Copy, Clone)]
struct Peak {
    hi: bool,
    dur: usize,
}

/// a HW5800 processor. Construct using ::new, then call
/// the add_sample function to provide it samples from the
/// radio. It will call the callback with HW5800Status
/// messages when they are detected.
pub struct HW5800<F: Fn(&HW5800Status) -> ()> {
    current: Vec<(f32, f32)>, // current list of raw samples
    max_count: usize, // number of samples to be averaged in the first pass
    buffer: VecDeque<f32>, // Contains averaged samples to be processed
    max_buffer: usize, // the size at which the buffer is processed
    threshold: f32,   // threshold for avg power for examining a buffer
    peak_dur: usize,  // The number of samples to count a peak
    lst: Peak,        // last seen peak.
    cur: Peak,        // current peak.
    on_cut: bool,     // tell if the last peak left us on cut or off cut
    msg: VecDeque<bool>, // bits of a potential message
    callback: F,      // the callback to be called with a status message
}

impl<F: Fn(&HW5800Status) -> ()> HW5800<F> {
    /// Create a HW5800 with default parameters.
    /// Callback will be called with a HW5800Status object
    /// containing the contents of the HW5800 message.
    pub fn new(callback: F) -> Self {
        // these parameters were determined by trial and
        // error on my device. YMMV.
        // If you're going to fiddle, you likely want max_count and peak_dur to
        // maintain approximately a 2:1 ratio.
        HW5800 {
            current: vec![(0., 0.)],
            max_count: 19,
            peak_dur: 10,
            buffer: VecDeque::new(),
            max_buffer: 128,
            threshold: 250.,
            lst: Peak { hi: true, dur: 0 },
            cur: Peak { hi: false, dur: 0 },
            on_cut: true,
            msg: VecDeque::new(),
            callback: callback,
        }
    }

    /// Present the next sample from the radio to the processing.
    /// Can cause a call to the HW5800's callback if a message is
    /// detected.
    pub fn add_sample(&mut self, real: f32, imag: f32) {
        self.current.push((real, imag));
        if self.current.len() == self.max_count {
            let r1 = self
                .current
                .iter()
                .fold((0., 0.), |a, b| (a.0 + b.0, a.1 + b.1));
            let l = self.current.len() as f32;
            self.averaged_sample((r1.0 / l).powi(2) + (r1.1 / l).powi(2));
            self.current.clear();
        }
    }

    fn averaged_sample(&mut self, sample: f32) {
        self.buffer.push_back(sample);
        if self.buffer.len() >= self.max_buffer {
            let avg: f32 =
                self.buffer.iter().sum::<f32>() / self.buffer.len() as f32;
            if avg < self.threshold {
                self.buffer.clear();
            } else {
                // full buffer. Compute the median and use that
                // to determine high/low freqs.
                let median =
                    self.buffer.iter().sum::<f32>() / self.buffer.len() as f32;
                while let Some(v) = self.buffer.pop_front() {
                    if (v > median) == self.cur.hi {
                        self.cur.dur += 1;
                    } else if self.cur.dur < 3 {
                        // cur spike is spurious
                        // XXX this could be more carefully thought through and
                        // might not even be a good idea...
                        self.lst.dur += self.cur.dur + 1;
                        self.cur = self.lst;
                    } else {
                        self.transition();
                        self.lst = self.cur;
                        self.cur.hi = v > median;
                        self.cur.dur = 1;
                    }
                }

                // Try to parse the message. We are looking for
                // a bit sequence that:
                // 1) starts with 0xFE
                // 2) passes the CRC check
                while self.msg.len() >= 7 * 8 {
                    // we need to start with a 1 bit. If not that,
                    // pop it and restart.
                    if self.msg.front() != Some(&true) {
                        self.msg.pop_front();
                        continue;
                    }
                    // likewise, we need to start with 0xfe
                    if self.message_begin() != 0xfe {
                        self.msg.pop_front();
                        continue;
                    }
                    // remove the 0xfe
                    for _ in 0..8 {
                        self.msg.pop_front();
                    }

                    // get the rest of the message
                    let m = self.message();
                    debug_assert_eq!(m.len(), 6);

                    // avoid the degerate case where the entire msg is 0
                    if m[4] == 0 && m[5] == 0 {
                        self.msg.pop_front();
                        continue;
                    }

                    // check the CRC
                    let c = crc(&m[..4]);
                    if m[4] == (c >> 8) as u8 && m[5] == (c & 0xff) as u8 {
                        info!("VALID: {}", ary_to_hex(&m));
                        let status = HW5800Status::new(&m);
                        (self.callback)(&status);
                        // remove the message
                        for _ in 0..(6 * 8) {
                            self.msg.pop_front();
                        }
                    } else {
                        //too noisy
                        //info!("CRC FAIL: {}", ary_to_hex(&m));
                        self.msg.pop_front();
                    }
                }
            }
        }
    }

    fn message_begin(&self) -> u8 {
        let mut acc = 0u8;
        for b in self.msg.iter().take(8) {
            acc = (acc << 1) + if *b { 1 } else { 0 };
        }
        return acc;
    }

    fn message(&self) -> Vec<u8> {
        let mut acc = 0u8;
        let mut ret = vec![];
        for (i, b) in self.msg.iter().enumerate().take(6 * 8) {
            acc = (acc << 1) + if *b { 1 } else { 0 };
            if i % 8 == 7 {
                ret.push(acc);
                acc = 0;
            }
        }
        if self.msg.len() % 8 != 0 {
            acc <<= 7 - (self.msg.len() % 8);
            ret.push(acc)
        }
        return ret;
    }

    // This is tricky.
    // A low to high transition is a 1
    // A high to low transition is a 0
    // Each sample is self.max_count us (~20).
    // A peak lasts about peak_dur (~10) samples (200 us).
    //
    // So we track if we are on cut and we have the following logic:
    //
    // - If we are on cut and we have a transition we can add
    //   the bit of the peak being transitioned to the the message.
    // - If we are not on cut we do not add to the message.
    //
    // Then we update on_cut as follows:
    // - if the message was less than peak_dur long, we negate it
    // - if we were not on cut, then it is some kind of error.
    // Presume the newest information is correct and set it as
    // though we were on cut.
    fn transition(&mut self) {
        if self.on_cut {
            self.msg.push_back(self.cur.hi);
        }
        if self.cur.dur < self.peak_dur {
            self.on_cut = !self.on_cut;
        } else if !self.on_cut {
            self.on_cut = true;
        }
    }
}
