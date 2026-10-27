//! Sensirion's VOC Index algorithm. This module ports the VOC path of the
//! float implementation in Sensirion's gas-index-algorithm, first released
//! in 3.2.1, at its default 1 s sampling interval, with the same f32
//! operations in the same order. The reference C in `tests/gas_index_ref/`
//! is identical to upstream `master`, whose header last changed in
//! f5addc5, although its version macro still reads "3.2.0".
//! `tests/gas_index.rs` checks the port sample for sample against that
//! reference C.
//!
//! It turns the SGP40's raw signal into the VOC Index, 1 to 500. An index
//! of 100 is this sensor's recent average air, and a higher index means
//! more VOCs than usual. Feed it one sample per second. It returns 0 for
//! the first 46 samples.
//!
//! Copyright (c) 2022, Sensirion AG
//! All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are met:
//!
//! * Redistributions of source code must retain the above copyright notice, this
//!   list of conditions and the following disclaimer.
//!
//! * Redistributions in binary form must reproduce the above copyright notice,
//!   this list of conditions and the following disclaimer in the documentation
//!   and/or other materials provided with the distribution.
//!
//! * Neither the name of Sensirion AG nor the names of its
//!   contributors may be used to endorse or promote products derived from
//!   this software without specific prior written permission.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//! AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//! IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
//! ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
//! LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
//! CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
//! SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
//! INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
//! CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
//! ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
//! POSSIBILITY OF SUCH DAMAGE.

const SAMPLING_INTERVAL: f32 = 1.0;
const INITIAL_BLACKOUT: f32 = 45.0;
const INDEX_GAIN: f32 = 230.0;
const SRAW_MINIMUM: i32 = 20000;
const SRAW_STD_INITIAL: f32 = 50.0;
const SRAW_STD_BONUS: f32 = 220.0;
const TAU_MEAN_HOURS: f32 = 12.0;
const TAU_VARIANCE_HOURS: f32 = 12.0;
const TAU_INITIAL_MEAN: f32 = 20.0;
const TAU_INITIAL_VARIANCE: f32 = 2500.0;
const INIT_DURATION_MEAN: f32 = 3600.0 * 0.75;
const INIT_DURATION_VARIANCE: f32 = 3600.0 * 1.45;
const INIT_TRANSITION_MEAN: f32 = 0.01;
const INIT_TRANSITION_VARIANCE: f32 = 0.01;
const GATING_THRESHOLD: f32 = 340.0;
const GATING_THRESHOLD_INITIAL: f32 = 510.0;
const GATING_THRESHOLD_TRANSITION: f32 = 0.09;
const GATING_MAX_DURATION_MINUTES: f32 = 60.0 * 3.0;
const GATING_MAX_RATIO: f32 = 0.3;
const SIGMOID_L: f32 = 500.0;
const SIGMOID_K: f32 = -0.0065;
const SIGMOID_X0: f32 = 213.0;
const LP_TAU_FAST: f32 = 20.0;
const LP_TAU_SLOW: f32 = 500.0;
const LP_ALPHA: f32 = -0.2;
const GAMMA_SCALING: f32 = 64.0;
const ADDITIONAL_GAMMA_MEAN_SCALING: f32 = 8.0;
const FIX16_MAX: f32 = 32767.0;
const PERSISTENCE_UPTIME_GAMMA: f32 = 3.0 * 3600.0;

const GAMMA_MEAN_BASE: f32 =
    ((ADDITIONAL_GAMMA_MEAN_SCALING * GAMMA_SCALING) * (SAMPLING_INTERVAL / 3600.0)) / (TAU_MEAN_HOURS + (SAMPLING_INTERVAL / 3600.0));
const GAMMA_VARIANCE_BASE: f32 = (GAMMA_SCALING * (SAMPLING_INTERVAL / 3600.0)) / (TAU_VARIANCE_HOURS + (SAMPLING_INTERVAL / 3600.0));
const GAMMA_INITIAL_MEAN: f32 =
    ((ADDITIONAL_GAMMA_MEAN_SCALING * GAMMA_SCALING) * SAMPLING_INTERVAL) / (TAU_INITIAL_MEAN + SAMPLING_INTERVAL);
const GAMMA_INITIAL_VARIANCE: f32 = (GAMMA_SCALING * SAMPLING_INTERVAL) / (TAU_INITIAL_VARIANCE + SAMPLING_INTERVAL);
const LP_A1: f32 = SAMPLING_INTERVAL / (LP_TAU_FAST + SAMPLING_INTERVAL);
const LP_A2: f32 = SAMPLING_INTERVAL / (LP_TAU_SLOW + SAMPLING_INTERVAL);

/// Logistic curve falling from `l` to 0 around `x0`. The estimator uses
/// `l = 1`; the index uses the reference's scaled sigmoid, whose shift term
/// is 0 and offset ratio 1 with the VOC defaults, so both of its branches
/// reduce to this with `l = SIGMOID_L`.
fn sigmoid(l: f32, k: f32, x0: f32, sample: f32) -> f32 {
    let x = k * (sample - x0);
    if x < -50.0 {
        l
    } else if x > 50.0 {
        0.0
    } else {
        l / (1.0 + x.exp())
    }
}

/// `GasIndexAlgorithmParams` for the VOC type. The reference's mox-model
/// copies of the estimator's mean and standard deviation always equal the
/// estimator state when read, so this port does not keep them separately.
#[derive(Debug, Clone)]
pub struct VocIndex {
    uptime: f32,
    sraw: f32,
    gas_index: f32,
    mve_initialized: bool,
    mve_mean: f32,
    mve_sraw_offset: f32,
    mve_std: f32,
    mve_gamma_mean: f32,
    mve_gamma_variance: f32,
    mve_uptime_gamma: f32,
    mve_uptime_gating: f32,
    mve_gating_duration_minutes: f32,
    /// Adaptive low-pass state (x1, x2, x3); None until the first sample.
    lp: Option<[f32; 3]>,
}

impl Default for VocIndex {
    fn default() -> VocIndex {
        VocIndex {
            uptime: 0.0,
            sraw: 0.0,
            gas_index: 0.0,
            mve_initialized: false,
            mve_mean: 0.0,
            mve_sraw_offset: 0.0,
            mve_std: SRAW_STD_INITIAL,
            mve_gamma_mean: 0.0,
            mve_gamma_variance: 0.0,
            mve_uptime_gamma: 0.0,
            mve_uptime_gating: 0.0,
            mve_gating_duration_minutes: 0.0,
            lp: None,
        }
    }
}

impl VocIndex {
    /// Takes one raw SGP40 sample and returns the VOC Index, or 0 during the
    /// blackout.
    pub fn process(&mut self, sraw: i32) -> i32 {
        if self.uptime <= INITIAL_BLACKOUT {
            self.uptime += SAMPLING_INTERVAL;
        } else {
            if sraw > 0 && sraw < 65000 {
                let sraw = sraw.clamp(SRAW_MINIMUM + 1, SRAW_MINIMUM + 32767);
                self.sraw = (sraw - SRAW_MINIMUM) as f32;
            }
            self.gas_index = self.lowpass(sigmoid(SIGMOID_L, SIGMOID_K, SIGMOID_X0, self.mox(self.sraw)));
            if self.gas_index < 0.5 {
                self.gas_index = 0.5;
            }
            if self.sraw > 0.0 {
                self.mve_process(self.sraw);
            }
        }
        (self.gas_index + 0.5) as i32
    }

    /// The learned baseline `(mean, std)` (the reference's
    /// `GasIndexAlgorithm_get_states`). Sensirion says it means something only
    /// after at least 3 h of continuous operation.
    pub fn state(&self) -> (f32, f32) {
        (self.mve_mean + self.mve_sraw_offset, self.mve_std)
    }

    /// Restores a baseline from [`VocIndex::state`] on a fresh instance
    /// (`GasIndexAlgorithm_set_states`), skipping the initial learning.
    /// Sensirion says not to restore one after an interruption of more than
    /// 10 minutes. The 45 s blackout still applies.
    pub fn set_state(&mut self, mean: f32, std: f32) {
        self.mve_mean = mean;
        self.mve_std = std;
        self.mve_uptime_gamma = PERSISTENCE_UPTIME_GAMMA;
        self.mve_initialized = true;
        self.sraw = mean;
    }

    fn mve_calculate_gamma(&mut self) {
        let uptime_limit = FIX16_MAX - SAMPLING_INTERVAL;
        if self.mve_uptime_gamma < uptime_limit {
            self.mve_uptime_gamma += SAMPLING_INTERVAL;
        }
        if self.mve_uptime_gating < uptime_limit {
            self.mve_uptime_gating += SAMPLING_INTERVAL;
        }
        let sigmoid_gamma_mean = sigmoid(1.0, INIT_TRANSITION_MEAN, INIT_DURATION_MEAN, self.mve_uptime_gamma);
        let gamma_mean = GAMMA_MEAN_BASE + ((GAMMA_INITIAL_MEAN - GAMMA_MEAN_BASE) * sigmoid_gamma_mean);
        let gating_threshold_mean = GATING_THRESHOLD
            + ((GATING_THRESHOLD_INITIAL - GATING_THRESHOLD)
                * sigmoid(1.0, INIT_TRANSITION_MEAN, INIT_DURATION_MEAN, self.mve_uptime_gating));
        let sigmoid_gating_mean = sigmoid(1.0, GATING_THRESHOLD_TRANSITION, gating_threshold_mean, self.gas_index);
        self.mve_gamma_mean = sigmoid_gating_mean * gamma_mean;

        let sigmoid_gamma_variance = sigmoid(1.0, INIT_TRANSITION_VARIANCE, INIT_DURATION_VARIANCE, self.mve_uptime_gamma);
        let gamma_variance =
            GAMMA_VARIANCE_BASE + ((GAMMA_INITIAL_VARIANCE - GAMMA_VARIANCE_BASE) * (sigmoid_gamma_variance - sigmoid_gamma_mean));
        let gating_threshold_variance = GATING_THRESHOLD
            + ((GATING_THRESHOLD_INITIAL - GATING_THRESHOLD)
                * sigmoid(1.0, INIT_TRANSITION_VARIANCE, INIT_DURATION_VARIANCE, self.mve_uptime_gating));
        let sigmoid_gating_variance = sigmoid(1.0, GATING_THRESHOLD_TRANSITION, gating_threshold_variance, self.gas_index);
        self.mve_gamma_variance = sigmoid_gating_variance * gamma_variance;

        self.mve_gating_duration_minutes +=
            (SAMPLING_INTERVAL / 60.0) * (((1.0 - sigmoid_gating_mean) * (1.0 + GATING_MAX_RATIO)) - GATING_MAX_RATIO);
        if self.mve_gating_duration_minutes < 0.0 {
            self.mve_gating_duration_minutes = 0.0;
        }
        if self.mve_gating_duration_minutes > GATING_MAX_DURATION_MINUTES {
            self.mve_uptime_gating = 0.0;
        }
    }

    fn mve_process(&mut self, sraw: f32) {
        if !self.mve_initialized {
            self.mve_initialized = true;
            self.mve_sraw_offset = sraw;
            self.mve_mean = 0.0;
            return;
        }
        if self.mve_mean >= 100.0 || self.mve_mean <= -100.0 {
            self.mve_sraw_offset += self.mve_mean;
            self.mve_mean = 0.0;
        }
        let sraw = sraw - self.mve_sraw_offset;
        self.mve_calculate_gamma();
        let delta_sgp = (sraw - self.mve_mean) / GAMMA_SCALING;
        let c = if delta_sgp < 0.0 { self.mve_std - delta_sgp } else { self.mve_std + delta_sgp };
        let additional_scaling = if c > 1440.0 { (c / 1440.0) * (c / 1440.0) } else { 1.0 };
        self.mve_std = (additional_scaling * (GAMMA_SCALING - self.mve_gamma_variance)).sqrt()
            * ((self.mve_std * (self.mve_std / (GAMMA_SCALING * additional_scaling)))
                + (((self.mve_gamma_variance * delta_sgp) / additional_scaling) * delta_sgp))
                .sqrt();
        self.mve_mean += (self.mve_gamma_mean * delta_sgp) / ADDITIONAL_GAMMA_MEAN_SCALING;
    }

    fn mox(&self, sraw: f32) -> f32 {
        ((sraw - (self.mve_mean + self.mve_sraw_offset)) / -(self.mve_std + SRAW_STD_BONUS)) * INDEX_GAIN
    }

    fn lowpass(&mut self, sample: f32) -> f32 {
        let [x1, x2, x3] = self.lp.unwrap_or([sample; 3]);
        let x1 = ((1.0 - LP_A1) * x1) + (LP_A1 * sample);
        let x2 = ((1.0 - LP_A2) * x2) + (LP_A2 * sample);
        let abs_delta = (x1 - x2).abs();
        let f1 = (LP_ALPHA * abs_delta).exp();
        let tau_a = ((LP_TAU_SLOW - LP_TAU_FAST) * f1) + LP_TAU_FAST;
        let a3 = SAMPLING_INTERVAL / (SAMPLING_INTERVAL + tau_a);
        let x3 = ((1.0 - a3) * x3) + (a3 * sample);
        self.lp = Some([x1, x2, x3]);
        x3
    }
}
