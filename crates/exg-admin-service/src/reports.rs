use exg_common::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeReport {
    pub period_start: UnixMicros,
    pub period_end: UnixMicros,
    pub total_maker_fees: Decimal128,
    pub total_taker_fees: Decimal128,
    pub total_funding_income: Decimal128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetReport {
    pub total_deposits: Decimal128,
    pub total_withdrawals: Decimal128,
    pub net_flow: Decimal128,
    pub deposit_count: u64,
    pub withdrawal_count: u64,
}

pub struct ReportGenerator {
    /// (timestamp, maker_fee, taker_fee)
    fee_records: Vec<(UnixMicros, Decimal128, Decimal128)>,
    /// (timestamp, funding_income)
    funding_records: Vec<(UnixMicros, Decimal128)>,
    deposit_total: Decimal128,
    withdrawal_total: Decimal128,
    deposit_count: u64,
    withdrawal_count: u64,
}

impl ReportGenerator {
    pub fn new() -> Self {
        Self {
            fee_records: Vec::new(),
            funding_records: Vec::new(),
            deposit_total: Decimal128::ZERO,
            withdrawal_total: Decimal128::ZERO,
            deposit_count: 0,
            withdrawal_count: 0,
        }
    }

    /// Record a fee event.
    pub fn record_fee(&mut self, maker: Decimal128, taker: Decimal128, timestamp: UnixMicros) {
        self.fee_records.push((timestamp, maker, taker));
    }

    /// Record funding income.
    pub fn record_funding_income(&mut self, amount: Decimal128, timestamp: UnixMicros) {
        self.funding_records.push((timestamp, amount));
    }

    /// Record a deposit.
    pub fn record_deposit(&mut self, amount: Decimal128) {
        self.deposit_total = self.deposit_total + amount;
        self.deposit_count += 1;
    }

    /// Record a withdrawal.
    pub fn record_withdrawal(&mut self, amount: Decimal128) {
        self.withdrawal_total = self.withdrawal_total + amount;
        self.withdrawal_count += 1;
    }

    /// Generate a fee report for the given time range `[start, end)`.
    pub fn generate_fee_report(&self, start: UnixMicros, end: UnixMicros) -> FeeReport {
        let mut total_maker = Decimal128::ZERO;
        let mut total_taker = Decimal128::ZERO;
        for &(ts, maker, taker) in &self.fee_records {
            if ts >= start && ts < end {
                total_maker = total_maker + maker;
                total_taker = total_taker + taker;
            }
        }

        let mut total_funding = Decimal128::ZERO;
        for &(ts, amount) in &self.funding_records {
            if ts >= start && ts < end {
                total_funding = total_funding + amount;
            }
        }

        FeeReport {
            period_start: start,
            period_end: end,
            total_maker_fees: total_maker,
            total_taker_fees: total_taker,
            total_funding_income: total_funding,
        }
    }

    /// Generate a cumulative asset report.
    pub fn generate_asset_report(&self) -> AssetReport {
        AssetReport {
            total_deposits: self.deposit_total,
            total_withdrawals: self.withdrawal_total,
            net_flow: self.deposit_total - self.withdrawal_total,
            deposit_count: self.deposit_count,
            withdrawal_count: self.withdrawal_count,
        }
    }
}

impl Default for ReportGenerator {
    fn default() -> Self {
        Self::new()
    }
}
