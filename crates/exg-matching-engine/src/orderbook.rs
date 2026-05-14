use std::cmp::Reverse;
use std::collections::BTreeMap;

use exg_common::{Decimal128, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// A single order on the book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookOrder {
    pub order_id: OrderId,
    pub user_id: UserId,
    pub symbol: SymbolId,
    pub side: Side,
    pub price: Decimal128,
    pub remaining_qty: Decimal128,
    pub original_qty: Decimal128,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub is_reduce_only: bool,
    pub timestamp: UnixMicros,
    /// Iceberg visible slice size.
    pub visible_qty: Option<Decimal128>,
    /// Remaining hidden quantity for iceberg orders.
    pub hidden_qty: Decimal128,
    /// Trailing stop delta.
    pub trailing_delta: Option<Decimal128>,
    /// Peak price tracked for trailing stop.
    pub trailing_peak_price: Option<Decimal128>,
    /// GTD expiry timestamp.
    pub expire_time: Option<UnixMicros>,
    pub client_order_id: Option<u64>,
    /// Stop/take-profit trigger price (for conditional orders).
    pub stop_price: Option<Decimal128>,
}

/// A (price, quantity) pair for depth display.
pub type DepthLevel = (Decimal128, Decimal128);

#[derive(Debug)]
pub struct PriceLevel {
    pub price: Decimal128,
    pub total_qty: Decimal128,
    pub orders: Vec<OrderId>, // FIFO order
}

/// The order book for a single symbol.
pub struct OrderBook {
    pub symbol: SymbolId,
    /// Bids sorted descending (highest price first) via Reverse key.
    bids: BTreeMap<Reverse<Decimal128>, PriceLevel>,
    /// Asks sorted ascending (lowest price first).
    asks: BTreeMap<Decimal128, PriceLevel>,
    /// O(1) lookup by order_id.
    orders: FxHashMap<OrderId, BookOrder>,
    /// User's active order IDs.
    user_orders: FxHashMap<UserId, Vec<OrderId>>,
}

impl OrderBook {
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: FxHashMap::default(),
            user_orders: FxHashMap::default(),
        }
    }

    /// Insert an order into the book (no matching, just placement).
    pub fn insert_order(&mut self, order: BookOrder) {
        let order_id = order.order_id;
        let user_id = order.user_id;
        let price = order.price;
        let side = order.side;
        let qty = order.remaining_qty;

        self.orders.insert(order_id, order);
        self.user_orders.entry(user_id).or_default().push(order_id);

        match side {
            Side::Buy => {
                let level = self
                    .bids
                    .entry(Reverse(price))
                    .or_insert_with(|| PriceLevel {
                        price,
                        total_qty: Decimal128::ZERO,
                        orders: Vec::new(),
                    });
                level.total_qty = level.total_qty + qty;
                level.orders.push(order_id);
            }
            Side::Sell => {
                let level = self.asks.entry(price).or_insert_with(|| PriceLevel {
                    price,
                    total_qty: Decimal128::ZERO,
                    orders: Vec::new(),
                });
                level.total_qty = level.total_qty + qty;
                level.orders.push(order_id);
            }
        }
    }

    /// Remove an order by ID. Returns the removed order if found.
    pub fn remove_order(&mut self, order_id: OrderId) -> Option<BookOrder> {
        let order = self.orders.remove(&order_id)?;

        // Remove from user_orders
        if let Some(user_ids) = self.user_orders.get_mut(&order.user_id) {
            user_ids.retain(|&id| id != order_id);
            if user_ids.is_empty() {
                self.user_orders.remove(&order.user_id);
            }
        }

        // Remove from price level
        match order.side {
            Side::Buy => {
                let key = Reverse(order.price);
                if let Some(level) = self.bids.get_mut(&key) {
                    level.orders.retain(|&id| id != order_id);
                    level.total_qty = level.total_qty - order.remaining_qty;
                    if level.orders.is_empty() {
                        self.bids.remove(&key);
                    }
                }
            }
            Side::Sell => {
                if let Some(level) = self.asks.get_mut(&order.price) {
                    level.orders.retain(|&id| id != order_id);
                    level.total_qty = level.total_qty - order.remaining_qty;
                    if level.orders.is_empty() {
                        self.asks.remove(&order.price);
                    }
                }
            }
        }

        Some(order)
    }

    /// Get an order by ID.
    pub fn get_order(&self, order_id: OrderId) -> Option<&BookOrder> {
        self.orders.get(&order_id)
    }

    /// Get a mutable reference to an order by ID.
    pub fn get_order_mut(&mut self, order_id: OrderId) -> Option<&mut BookOrder> {
        self.orders.get_mut(&order_id)
    }

    /// Get all order IDs for a user.
    pub fn get_user_orders(&self, user_id: UserId) -> &[OrderId] {
        self.user_orders
            .get(&user_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Cancel all orders for a user. Returns canceled orders.
    pub fn cancel_all_user_orders(&mut self, user_id: UserId) -> Vec<BookOrder> {
        let order_ids = match self.user_orders.remove(&user_id) {
            Some(ids) => ids,
            None => return Vec::new(),
        };

        let mut canceled = Vec::with_capacity(order_ids.len());
        for order_id in order_ids {
            if let Some(order) = self.orders.remove(&order_id) {
                // Remove from price level
                match order.side {
                    Side::Buy => {
                        let key = Reverse(order.price);
                        if let Some(level) = self.bids.get_mut(&key) {
                            level.orders.retain(|&id| id != order_id);
                            level.total_qty = level.total_qty - order.remaining_qty;
                            if level.orders.is_empty() {
                                self.bids.remove(&key);
                            }
                        }
                    }
                    Side::Sell => {
                        if let Some(level) = self.asks.get_mut(&order.price) {
                            level.orders.retain(|&id| id != order_id);
                            level.total_qty = level.total_qty - order.remaining_qty;
                            if level.orders.is_empty() {
                                self.asks.remove(&order.price);
                            }
                        }
                    }
                }
                canceled.push(order);
            }
        }
        canceled
    }

    /// Best bid price.
    pub fn best_bid(&self) -> Option<Decimal128> {
        self.bids.first_key_value().map(|(Reverse(p), _)| *p)
    }

    /// Best ask price.
    pub fn best_ask(&self) -> Option<Decimal128> {
        self.asks.first_key_value().map(|(p, _)| *p)
    }

    /// Get top N price levels for each side.
    /// Returns (bids, asks) where each entry is (price, total_qty).
    pub fn depth(&self, levels: usize) -> (Vec<DepthLevel>, Vec<DepthLevel>) {
        let bids: Vec<_> = self
            .bids
            .iter()
            .take(levels)
            .map(|(_, level)| (level.price, level.total_qty))
            .collect();
        let asks: Vec<_> = self
            .asks
            .iter()
            .take(levels)
            .map(|(_, level)| (level.price, level.total_qty))
            .collect();
        (bids, asks)
    }

    /// Update remaining quantity of an existing order (for partial fills).
    pub fn update_qty(&mut self, order_id: OrderId, new_remaining: Decimal128) {
        if let Some(order) = self.orders.get_mut(&order_id) {
            let old_remaining = order.remaining_qty;
            let diff = old_remaining - new_remaining;
            order.remaining_qty = new_remaining;

            match order.side {
                Side::Buy => {
                    if let Some(level) = self.bids.get_mut(&Reverse(order.price)) {
                        level.total_qty = level.total_qty - diff;
                    }
                }
                Side::Sell => {
                    if let Some(level) = self.asks.get_mut(&order.price) {
                        level.total_qty = level.total_qty - diff;
                    }
                }
            }
        }
    }

    /// Total number of orders on the book.
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }

    /// Check if book is empty.
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Iterator over ask price levels in ascending order.
    /// Used by matcher to walk asks for a buy order.
    pub(crate) fn ask_levels(&self) -> impl Iterator<Item = &PriceLevel> {
        self.asks.values()
    }

    /// Iterator over bid price levels in descending order.
    /// Used by matcher to walk bids for a sell order.
    pub(crate) fn bid_levels(&self) -> impl Iterator<Item = &PriceLevel> {
        self.bids.values()
    }

    /// Get all orders on the book (for snapshot).
    pub fn all_orders(&self) -> impl Iterator<Item = &BookOrder> {
        self.orders.values()
    }

    /// Check available quantity at or better than a given price for FOK validation.
    pub fn available_qty_at_price(&self, side: Side, limit_price: Decimal128) -> Decimal128 {
        match side {
            // Buy taker matches against asks: all ask levels with price <= limit_price
            Side::Buy => {
                let mut total = Decimal128::ZERO;
                for (_, level) in self.asks.iter() {
                    if level.price > limit_price {
                        break;
                    }
                    total = total + level.total_qty;
                }
                total
            }
            // Sell taker matches against bids: all bid levels with price >= limit_price
            Side::Sell => {
                let mut total = Decimal128::ZERO;
                for (_, level) in self.bids.iter() {
                    if level.price < limit_price {
                        break;
                    }
                    total = total + level.total_qty;
                }
                total
            }
        }
    }

    /// Total available quantity on the opposite side (for market FOK).
    pub fn total_opposite_qty(&self, side: Side) -> Decimal128 {
        match side {
            Side::Buy => self.asks.values().map(|l| l.total_qty).sum(),
            Side::Sell => self.bids.values().map(|l| l.total_qty).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn make_order(id: u64, user: u64, side: Side, price: &str, qty: &str) -> BookOrder {
        BookOrder {
            order_id: OrderId::new(id),
            user_id: UserId::new(user),
            symbol: SymbolId::new(1),
            side,
            price: dec(price),
            remaining_qty: dec(qty),
            original_qty: dec(qty),
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            is_reduce_only: false,
            timestamp: UnixMicros::from_micros(1_000_000 + id),
            visible_qty: None,
            hidden_qty: Decimal128::ZERO,
            trailing_delta: None,
            trailing_peak_price: None,
            expire_time: None,
            client_order_id: None,
            stop_price: None,
        }
    }

    // 1. Insert and retrieve order
    #[test]
    fn test_insert_and_retrieve() {
        let mut book = OrderBook::new(SymbolId::new(1));
        let order = make_order(1, 10, Side::Buy, "50000", "1");
        book.insert_order(order.clone());

        let retrieved = book.get_order(OrderId::new(1)).unwrap();
        assert_eq!(retrieved.order_id, OrderId::new(1));
        assert_eq!(retrieved.price, dec("50000"));
        assert_eq!(retrieved.remaining_qty, dec("1"));
        assert_eq!(book.order_count(), 1);
    }

    // 2. Remove order — verify price level cleaned up when empty
    #[test]
    fn test_remove_order_cleans_price_level() {
        let mut book = OrderBook::new(SymbolId::new(1));
        book.insert_order(make_order(1, 10, Side::Buy, "50000", "1"));
        book.insert_order(make_order(2, 10, Side::Buy, "50000", "2"));

        // Remove one — level should still exist
        let removed = book.remove_order(OrderId::new(1)).unwrap();
        assert_eq!(removed.remaining_qty, dec("1"));
        assert!(book.best_bid().is_some());

        // Remove second — level should be cleaned up
        book.remove_order(OrderId::new(2));
        assert!(book.best_bid().is_none());
        assert!(book.is_empty());
    }

    // 3. Best bid/ask correct after insertions
    #[test]
    fn test_best_bid_ask() {
        let mut book = OrderBook::new(SymbolId::new(1));
        book.insert_order(make_order(1, 10, Side::Buy, "49000", "1"));
        book.insert_order(make_order(2, 11, Side::Buy, "50000", "1"));
        book.insert_order(make_order(3, 12, Side::Sell, "51000", "1"));
        book.insert_order(make_order(4, 13, Side::Sell, "52000", "1"));

        assert_eq!(book.best_bid(), Some(dec("50000")));
        assert_eq!(book.best_ask(), Some(dec("51000")));
    }

    // 4. Cancel all user orders
    #[test]
    fn test_cancel_all_user_orders() {
        let mut book = OrderBook::new(SymbolId::new(1));
        book.insert_order(make_order(1, 10, Side::Buy, "49000", "1"));
        book.insert_order(make_order(2, 10, Side::Sell, "51000", "2"));
        book.insert_order(make_order(3, 11, Side::Buy, "48000", "3"));

        let canceled = book.cancel_all_user_orders(UserId::new(10));
        assert_eq!(canceled.len(), 2);
        assert_eq!(book.order_count(), 1);
        assert!(book.get_order(OrderId::new(3)).is_some());
        assert!(book.get_user_orders(UserId::new(10)).is_empty());
    }

    // 5. Depth query returns correct levels
    #[test]
    fn test_depth() {
        let mut book = OrderBook::new(SymbolId::new(1));
        book.insert_order(make_order(1, 10, Side::Buy, "49000", "1"));
        book.insert_order(make_order(2, 11, Side::Buy, "50000", "2"));
        book.insert_order(make_order(3, 12, Side::Buy, "50000", "3"));
        book.insert_order(make_order(4, 13, Side::Sell, "51000", "1"));
        book.insert_order(make_order(5, 14, Side::Sell, "52000", "4"));

        let (bids, asks) = book.depth(5);

        // Bids: 50000 (qty=5), 49000 (qty=1) — descending
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0], (dec("50000"), dec("5")));
        assert_eq!(bids[1], (dec("49000"), dec("1")));

        // Asks: 51000 (qty=1), 52000 (qty=4) — ascending
        assert_eq!(asks.len(), 2);
        assert_eq!(asks[0], (dec("51000"), dec("1")));
        assert_eq!(asks[1], (dec("52000"), dec("4")));
    }

    // 6. Large book insert/remove
    #[test]
    fn test_large_book_performance() {
        let mut book = OrderBook::new(SymbolId::new(1));
        for i in 0..1000 {
            let price = format!("{}", 50000 + (i % 100));
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            book.insert_order(make_order(i, i % 50, side, &price, "1"));
        }
        assert_eq!(book.order_count(), 1000);

        // Remove half
        for i in 0..500 {
            book.remove_order(OrderId::new(i));
        }
        assert_eq!(book.order_count(), 500);
    }

    #[test]
    fn test_update_qty() {
        let mut book = OrderBook::new(SymbolId::new(1));
        book.insert_order(make_order(1, 10, Side::Buy, "50000", "10"));

        book.update_qty(OrderId::new(1), dec("7"));

        let order = book.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("7"));

        let (bids, _) = book.depth(1);
        assert_eq!(bids[0].1, dec("7"));
    }

    #[test]
    fn test_empty_book() {
        let book = OrderBook::new(SymbolId::new(1));
        assert!(book.is_empty());
        assert_eq!(book.order_count(), 0);
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
        let (bids, asks) = book.depth(10);
        assert!(bids.is_empty());
        assert!(asks.is_empty());
    }

    #[test]
    fn test_get_user_orders_empty() {
        let book = OrderBook::new(SymbolId::new(1));
        assert!(book.get_user_orders(UserId::new(999)).is_empty());
    }
}
