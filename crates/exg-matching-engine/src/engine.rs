use std::cmp::Reverse;
use std::collections::BinaryHeap;

use exg_common::{
    Decimal128, OrderId, OrderType, Side, SnowflakeGen, SymbolId, TimeInForce, TradeId, UnixMicros,
    UserId,
};
use exg_protocol::{Command, Event, RejectReason};
use exg_risk_engine::SymbolConfig;

use crate::matcher::{self, Fill};
use crate::orderbook::{BookOrder, OrderBook};
use crate::snapshot::EngineSnapshot;

/// The main matching engine for a single symbol.
pub struct MatchingEngine {
    orderbook: OrderBook,
    symbol_config: SymbolConfig,
    mark_price: Decimal128,
    index_price: Decimal128,
    /// Conditional orders waiting for trigger (stop/take-profit/trailing).
    stop_orders: Vec<BookOrder>,
    /// GTD expiry tracking: min-heap by (expire_time, order_id).
    expiry_heap: BinaryHeap<Reverse<(UnixMicros, OrderId)>>,
    /// Trade ID generator.
    trade_id_gen: SnowflakeGen,
    /// Sequence counter for WAL ordering.
    sequence: u64,
}

impl MatchingEngine {
    pub fn new(symbol_config: SymbolConfig, node_id: u16) -> Self {
        let symbol = symbol_config.symbol;
        Self {
            orderbook: OrderBook::new(symbol),
            symbol_config,
            mark_price: Decimal128::ZERO,
            index_price: Decimal128::ZERO,
            stop_orders: Vec::new(),
            expiry_heap: BinaryHeap::new(),
            trade_id_gen: SnowflakeGen::new(node_id),
            sequence: 0,
        }
    }

    /// Process a single command. Returns generated events.
    pub fn process_command(&mut self, cmd: &Command) -> Vec<Event> {
        self.sequence += 1;
        match cmd {
            Command::NewOrder {
                order_id,
                user_id,
                symbol,
                side,
                order_type,
                time_in_force,
                price,
                quantity,
                stop_price,
                trailing_delta,
                visible_quantity,
                reduce_only,
                timestamp,
                client_order_id,
                ..
            } => self.handle_new_order(
                *order_id,
                *user_id,
                *symbol,
                *side,
                *order_type,
                *time_in_force,
                *price,
                *quantity,
                *stop_price,
                *trailing_delta,
                *visible_quantity,
                *reduce_only,
                *timestamp,
                *client_order_id,
            ),
            Command::CancelOrder {
                order_id,
                user_id,
                timestamp,
                ..
            } => self.handle_cancel_order(*order_id, *user_id, *timestamp),
            Command::AmendOrder {
                order_id,
                user_id,
                symbol,
                new_price,
                new_quantity,
                timestamp,
            } => self.handle_amend_order(
                *order_id,
                *user_id,
                *symbol,
                *new_price,
                *new_quantity,
                *timestamp,
            ),
            Command::CancelAllOrders {
                user_id,
                symbol,
                timestamp,
            } => self.handle_cancel_all(*user_id, *symbol, *timestamp),
        }
    }

    /// Process NewOrder command.
    #[allow(clippy::too_many_arguments)]
    fn handle_new_order(
        &mut self,
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        side: Side,
        order_type: OrderType,
        time_in_force: TimeInForce,
        price: Option<Decimal128>,
        quantity: Decimal128,
        stop_price: Option<Decimal128>,
        trailing_delta: Option<Decimal128>,
        visible_quantity: Option<Decimal128>,
        reduce_only: bool,
        timestamp: UnixMicros,
        client_order_id: Option<u64>,
    ) -> Vec<Event> {
        let mut events = Vec::new();

        // Basic validation
        if quantity.is_zero() || quantity.is_negative() {
            events.push(Event::OrderRejected {
                order_id,
                user_id,
                reason: RejectReason::InvalidOrder,
                timestamp,
            });
            return events;
        }

        // Duplicate check
        if self.orderbook.get_order(order_id).is_some() {
            events.push(Event::OrderRejected {
                order_id,
                user_id,
                reason: RejectReason::DuplicateOrder,
                timestamp,
            });
            return events;
        }

        // Validate limit price
        let effective_price = match order_type {
            OrderType::Limit | OrderType::StopLimit | OrderType::TakeProfitLimit | OrderType::Iceberg => {
                match price {
                    Some(p) if p.is_positive() => p,
                    _ => {
                        events.push(Event::OrderRejected {
                            order_id,
                            user_id,
                            reason: RejectReason::InvalidOrder,
                            timestamp,
                        });
                        return events;
                    }
                }
            }
            // Market-like orders use Decimal128::MAX for buy, ZERO for sell as sentinel
            OrderType::Market | OrderType::StopMarket | OrderType::TakeProfitMarket | OrderType::TrailingStop => {
                match side {
                    Side::Buy => Decimal128::MAX,
                    Side::Sell => Decimal128::ZERO,
                }
            }
        };

        // Validate iceberg visible_qty
        if order_type == OrderType::Iceberg {
            if let Some(vis) = visible_quantity {
                let min_visible = self.symbol_config.lot_size * Decimal128::from(10i64);
                if vis < min_visible {
                    events.push(Event::OrderRejected {
                        order_id,
                        user_id,
                        reason: RejectReason::InvalidOrder,
                        timestamp,
                    });
                    return events;
                }
            } else {
                events.push(Event::OrderRejected {
                    order_id,
                    user_id,
                    reason: RejectReason::InvalidOrder,
                    timestamp,
                });
                return events;
            }
        }

        // Build BookOrder
        let (remaining_qty, hidden_qty) = if order_type == OrderType::Iceberg {
            let vis = visible_quantity.unwrap();
            let visible = vis.min(quantity);
            let hidden = quantity - visible;
            (visible, hidden)
        } else {
            (quantity, Decimal128::ZERO)
        };

        let expire_time = if time_in_force == TimeInForce::Gtd {
            // Default GTD expiry: 24 hours from order timestamp.
            // In production, this would come from an explicit field on the command.
            let twenty_four_hours_micros: u64 = 24 * 3600 * 1_000_000;
            Some(UnixMicros::from_micros(
                timestamp.as_micros() + twenty_four_hours_micros,
            ))
        } else {
            None
        };

        let mut book_order = BookOrder {
            order_id,
            user_id,
            symbol,
            side,
            price: effective_price,
            remaining_qty,
            original_qty: quantity,
            order_type,
            time_in_force,
            is_reduce_only: reduce_only,
            timestamp,
            visible_qty: visible_quantity,
            hidden_qty,
            trailing_delta,
            trailing_peak_price: if trailing_delta.is_some() {
                Some(self.mark_price)
            } else {
                None
            },
            expire_time,
            client_order_id,
            stop_price,
        };

        // Conditional orders: queue for trigger instead of matching
        if order_type.is_conditional() {
            events.push(Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id,
                timestamp,
            });
            self.stop_orders.push(book_order);
            return events;
        }

        // Accept event before matching
        events.push(Event::OrderAccepted {
            order_id,
            user_id,
            symbol,
            client_order_id,
            timestamp,
        });

        // Match against the book
        let match_result = matcher::match_order(&mut self.orderbook, &mut book_order);

        if match_result.rejected {
            // POST_ONLY would take or FOK not fillable
            let reason = if time_in_force == TimeInForce::PostOnly {
                RejectReason::PostOnlyWouldTake
            } else {
                RejectReason::FokNotFillable
            };
            // Replace OrderAccepted with OrderRejected
            events.clear();
            events.push(Event::OrderRejected {
                order_id,
                user_id,
                reason,
                timestamp,
            });
            return events;
        }

        // Emit fill events
        for fill in &match_result.fills {
            let trade_id = TradeId::new(self.trade_id_gen.next_id());

            let (buyer_order_id, seller_order_id, buyer_user_id, seller_user_id) =
                match fill.taker_side {
                    Side::Buy => (
                        fill.taker_order_id,
                        fill.maker_order_id,
                        fill.taker_user_id,
                        fill.maker_user_id,
                    ),
                    Side::Sell => (
                        fill.maker_order_id,
                        fill.taker_order_id,
                        fill.maker_user_id,
                        fill.taker_user_id,
                    ),
                };

            let notional = fill.price * fill.quantity;
            let maker_fee = notional * self.symbol_config.maker_fee;
            let taker_fee = notional * self.symbol_config.taker_fee;

            let (buyer_fee, seller_fee) = match fill.taker_side {
                Side::Buy => (taker_fee, maker_fee),
                Side::Sell => (maker_fee, taker_fee),
            };

            // Maker fill event
            let maker_remaining = self
                .orderbook
                .get_order(fill.maker_order_id)
                .map(|o| o.remaining_qty)
                .unwrap_or(Decimal128::ZERO);

            events.push(Event::OrderFilled {
                order_id: fill.maker_order_id,
                trade_id,
                user_id: fill.maker_user_id,
                symbol,
                side: fill.taker_side.opposite(),
                fill_price: fill.price,
                fill_qty: fill.quantity,
                is_maker: true,
                remaining_qty: maker_remaining,
                timestamp,
            });

            // Taker fill event
            events.push(Event::OrderFilled {
                order_id: fill.taker_order_id,
                trade_id,
                user_id: fill.taker_user_id,
                symbol,
                side: fill.taker_side,
                fill_price: fill.price,
                fill_qty: fill.quantity,
                is_maker: false,
                remaining_qty: book_order.remaining_qty,
                timestamp,
            });

            // Trade event
            events.push(Event::TradeExecuted {
                trade_id,
                symbol,
                price: fill.price,
                qty: fill.quantity,
                buyer_order_id,
                seller_order_id,
                buyer_user_id,
                seller_user_id,
                buyer_fee,
                seller_fee,
                timestamp,
            });
        }

        // Handle iceberg refill after matching
        if order_type == OrderType::Iceberg
            && book_order.remaining_qty.is_zero()
            && !book_order.hidden_qty.is_zero()
        {
            let vis = book_order.visible_qty.unwrap_or(book_order.original_qty);
            let refill = vis.min(book_order.hidden_qty);
            book_order.hidden_qty = book_order.hidden_qty - refill;
            book_order.remaining_qty = refill;
            // Timestamp updated on refill = loss of time priority
            book_order.timestamp = timestamp;
        }

        // Place remaining quantity on book (if applicable)
        if !book_order.remaining_qty.is_zero() {
            match time_in_force {
                TimeInForce::Ioc => {
                    // Cancel remaining
                    events.push(Event::OrderCanceled {
                        order_id,
                        user_id,
                        symbol,
                        remaining_qty: book_order.remaining_qty,
                        timestamp,
                    });
                }
                TimeInForce::Fok => {
                    // Should not reach here — FOK that partially fills is rejected above
                    // But if somehow remaining, cancel it
                    events.push(Event::OrderCanceled {
                        order_id,
                        user_id,
                        symbol,
                        remaining_qty: book_order.remaining_qty,
                        timestamp,
                    });
                }
                _ => {
                    // GTC, GTD, PostOnly — rest on book
                    if order_type.is_limit() || order_type == OrderType::Iceberg {
                        if let Some(expire) = book_order.expire_time {
                            self.expiry_heap
                                .push(Reverse((expire, order_id)));
                        }
                        self.orderbook.insert_order(book_order);
                    } else {
                        // Market order with no more depth — cancel remaining
                        events.push(Event::OrderCanceled {
                            order_id,
                            user_id,
                            symbol,
                            remaining_qty: book_order.remaining_qty,
                            timestamp,
                        });
                    }
                }
            }
        }

        events
    }

    /// Process CancelOrder command.
    fn handle_cancel_order(
        &mut self,
        order_id: OrderId,
        user_id: UserId,
        timestamp: UnixMicros,
    ) -> Vec<Event> {
        // Check the order book first
        if let Some(order) = self.orderbook.remove_order(order_id) {
            if order.user_id != user_id {
                // Put it back — wrong user
                self.orderbook.insert_order(order);
                return vec![Event::OrderRejected {
                    order_id,
                    user_id,
                    reason: RejectReason::OrderNotFound,
                    timestamp,
                }];
            }
            return vec![Event::OrderCanceled {
                order_id,
                user_id,
                symbol: order.symbol,
                remaining_qty: order.remaining_qty,
                timestamp,
            }];
        }

        // Check stop orders
        if let Some(pos) = self.stop_orders.iter().position(|o| o.order_id == order_id) {
            let order = self.stop_orders.remove(pos);
            if order.user_id != user_id {
                self.stop_orders.push(order);
                return vec![Event::OrderRejected {
                    order_id,
                    user_id,
                    reason: RejectReason::OrderNotFound,
                    timestamp,
                }];
            }
            return vec![Event::OrderCanceled {
                order_id,
                user_id,
                symbol: order.symbol,
                remaining_qty: order.remaining_qty,
                timestamp,
            }];
        }

        vec![Event::OrderRejected {
            order_id,
            user_id,
            reason: RejectReason::OrderNotFound,
            timestamp,
        }]
    }

    /// Process AmendOrder command.
    fn handle_amend_order(
        &mut self,
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        new_price: Option<Decimal128>,
        new_quantity: Option<Decimal128>,
        timestamp: UnixMicros,
    ) -> Vec<Event> {
        let existing = self.orderbook.get_order(order_id);
        if existing.is_none() {
            return vec![Event::OrderRejected {
                order_id,
                user_id,
                reason: RejectReason::OrderNotFound,
                timestamp,
            }];
        }
        let existing = existing.unwrap();
        if existing.user_id != user_id {
            return vec![Event::OrderRejected {
                order_id,
                user_id,
                reason: RejectReason::OrderNotFound,
                timestamp,
            }];
        }

        let price_changed = new_price.is_some_and(|p| p != existing.price);
        let qty_down = new_quantity.is_some_and(|q| q < existing.remaining_qty);
        let qty_up = new_quantity.is_some_and(|q| q > existing.remaining_qty);

        // Price change or qty increase → cancel + re-insert (loses time priority)
        if price_changed || qty_up {
            let old = self.orderbook.remove_order(order_id).unwrap();
            let mut events = vec![Event::OrderCanceled {
                order_id,
                user_id,
                symbol,
                remaining_qty: old.remaining_qty,
                timestamp,
            }];

            let new_p = new_price.unwrap_or(old.price);
            let new_q = new_quantity.unwrap_or(old.remaining_qty);

            if new_q.is_zero() || new_q.is_negative() || new_p.is_zero() || new_p.is_negative() {
                events.push(Event::OrderRejected {
                    order_id,
                    user_id,
                    reason: RejectReason::InvalidOrder,
                    timestamp,
                });
                return events;
            }

            let mut amended = old;
            amended.price = new_p;
            amended.remaining_qty = new_q;
            amended.original_qty = new_q;
            amended.timestamp = timestamp;

            // Try to match the amended order
            let match_result = matcher::match_order(&mut self.orderbook, &mut amended);

            if match_result.rejected {
                let reason = if amended.time_in_force == TimeInForce::PostOnly {
                    RejectReason::PostOnlyWouldTake
                } else {
                    RejectReason::FokNotFillable
                };
                events.push(Event::OrderRejected {
                    order_id,
                    user_id,
                    reason,
                    timestamp,
                });
                return events;
            }

            // Accept the amended order
            events.push(Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: amended.client_order_id,
                timestamp,
            });

            // Emit fill events
            self.emit_fill_events(&mut events, &match_result.fills, symbol, timestamp, &amended);

            // Rest remaining on book
            if !amended.remaining_qty.is_zero() && amended.order_type.is_limit() {
                self.orderbook.insert_order(amended);
            }

            events
        } else if qty_down {
            // Quantity decrease only — in-place modify (keeps time priority)
            let new_q = new_quantity.unwrap();
            if new_q.is_zero() || new_q.is_negative() {
                return vec![Event::OrderRejected {
                    order_id,
                    user_id,
                    reason: RejectReason::InvalidOrder,
                    timestamp,
                }];
            }
            self.orderbook.update_qty(order_id, new_q);

            vec![Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: self
                    .orderbook
                    .get_order(order_id)
                    .and_then(|o| o.client_order_id),
                timestamp,
            }]
        } else {
            // No meaningful change
            vec![Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id: existing.client_order_id,
                timestamp,
            }]
        }
    }

    /// Process CancelAllOrders command.
    fn handle_cancel_all(
        &mut self,
        user_id: UserId,
        symbol: SymbolId,
        timestamp: UnixMicros,
    ) -> Vec<Event> {
        let mut events = Vec::new();

        // Cancel book orders
        let canceled = self.orderbook.cancel_all_user_orders(user_id);
        for order in canceled {
            events.push(Event::OrderCanceled {
                order_id: order.order_id,
                user_id,
                symbol,
                remaining_qty: order.remaining_qty,
                timestamp,
            });
        }

        // Cancel stop orders for this user+symbol
        let mut i = 0;
        while i < self.stop_orders.len() {
            if self.stop_orders[i].user_id == user_id && self.stop_orders[i].symbol == symbol {
                let order = self.stop_orders.swap_remove(i);
                events.push(Event::OrderCanceled {
                    order_id: order.order_id,
                    user_id,
                    symbol,
                    remaining_qty: order.remaining_qty,
                    timestamp,
                });
            } else {
                i += 1;
            }
        }

        events
    }

    /// Update mark/index price. Check stop orders for triggering.
    pub fn update_mark_price(
        &mut self,
        mark_price: Decimal128,
        index_price: Decimal128,
    ) -> Vec<Event> {
        self.mark_price = mark_price;
        self.index_price = index_price;

        let mut events = Vec::new();

        // Update trailing peak prices
        self.update_trailing_peaks();

        // Check stop triggers
        let mut triggered = self.check_stop_triggers_internal();

        // Process triggered orders through the book
        for mut order in triggered.drain(..) {
            // Convert to market or limit depending on type
            match order.order_type {
                OrderType::StopMarket | OrderType::TakeProfitMarket | OrderType::TrailingStop => {
                    order.order_type = OrderType::Market;
                    order.price = match order.side {
                        Side::Buy => Decimal128::MAX,
                        Side::Sell => Decimal128::ZERO,
                    };
                }
                OrderType::StopLimit | OrderType::TakeProfitLimit => {
                    order.order_type = OrderType::Limit;
                    // price stays as the limit price
                }
                _ => {}
            }

            let timestamp = UnixMicros::now();
            let match_result = matcher::match_order(&mut self.orderbook, &mut order);

            if !match_result.rejected {
                self.emit_fill_events(
                    &mut events,
                    &match_result.fills,
                    order.symbol,
                    timestamp,
                    &order,
                );

                if !order.remaining_qty.is_zero() && order.order_type.is_limit() {
                    self.orderbook.insert_order(order);
                }
            }
        }

        events
    }

    /// Check and expire GTD orders.
    pub fn check_expirations(&mut self, now: UnixMicros) -> Vec<Event> {
        let mut events = Vec::new();

        while let Some(&Reverse((expire_time, order_id))) = self.expiry_heap.peek() {
            if expire_time > now {
                break;
            }
            self.expiry_heap.pop();

            if let Some(order) = self.orderbook.remove_order(order_id) {
                events.push(Event::OrderCanceled {
                    order_id,
                    user_id: order.user_id,
                    symbol: order.symbol,
                    remaining_qty: order.remaining_qty,
                    timestamp: now,
                });
            }
        }

        events
    }

    /// Update trailing peak prices for trailing stop orders.
    fn update_trailing_peaks(&mut self) {
        for order in &mut self.stop_orders {
            if order.order_type == OrderType::TrailingStop
                && let Some(ref mut peak) = order.trailing_peak_price
            {
                match order.side {
                    // Trailing stop sell: tracks upward peak (sell trigger on reversal down)
                    Side::Sell => {
                        if self.mark_price > *peak {
                            *peak = self.mark_price;
                        }
                    }
                    // Trailing stop buy: tracks downward trough (buy trigger on reversal up)
                    Side::Buy => {
                        if self.mark_price < *peak {
                            *peak = self.mark_price;
                        }
                    }
                }
            }
        }
    }

    /// Check stop/take-profit triggers. Returns orders that should be activated.
    fn check_stop_triggers_internal(&mut self) -> Vec<BookOrder> {
        let mut triggered = Vec::new();
        let mark = self.mark_price;

        let mut i = 0;
        while i < self.stop_orders.len() {
            let should_trigger = match self.stop_orders[i].order_type {
                OrderType::StopMarket | OrderType::StopLimit => {
                    let stop = self.stop_orders[i].stop_price.unwrap_or(Decimal128::ZERO);
                    match self.stop_orders[i].side {
                        // STOP buy: triggers when mark >= stop
                        Side::Buy => mark >= stop,
                        // STOP sell: triggers when mark <= stop
                        Side::Sell => mark <= stop,
                    }
                }
                OrderType::TakeProfitMarket | OrderType::TakeProfitLimit => {
                    let stop = self.stop_orders[i].stop_price.unwrap_or(Decimal128::ZERO);
                    match self.stop_orders[i].side {
                        // TP buy: triggers when mark <= stop
                        Side::Buy => mark <= stop,
                        // TP sell: triggers when mark >= stop
                        Side::Sell => mark >= stop,
                    }
                }
                OrderType::TrailingStop => {
                    if let (Some(delta), Some(peak)) = (
                        self.stop_orders[i].trailing_delta,
                        self.stop_orders[i].trailing_peak_price,
                    ) {
                        match self.stop_orders[i].side {
                            // Trailing stop sell: trigger when mark drops delta below peak
                            Side::Sell => mark <= peak - delta,
                            // Trailing stop buy: trigger when mark rises delta above trough
                            Side::Buy => mark >= peak + delta,
                        }
                    } else {
                        false
                    }
                }
                _ => false,
            };

            if should_trigger {
                triggered.push(self.stop_orders.swap_remove(i));
            } else {
                i += 1;
            }
        }

        triggered
    }

    /// Emit fill and trade events from fills.
    fn emit_fill_events(
        &mut self,
        events: &mut Vec<Event>,
        fills: &[Fill],
        symbol: SymbolId,
        timestamp: UnixMicros,
        taker: &BookOrder,
    ) {
        for fill in fills {
            let trade_id = TradeId::new(self.trade_id_gen.next_id());

            let (buyer_order_id, seller_order_id, buyer_user_id, seller_user_id) =
                match fill.taker_side {
                    Side::Buy => (
                        fill.taker_order_id,
                        fill.maker_order_id,
                        fill.taker_user_id,
                        fill.maker_user_id,
                    ),
                    Side::Sell => (
                        fill.maker_order_id,
                        fill.taker_order_id,
                        fill.maker_user_id,
                        fill.taker_user_id,
                    ),
                };

            let notional = fill.price * fill.quantity;
            let maker_fee = notional * self.symbol_config.maker_fee;
            let taker_fee = notional * self.symbol_config.taker_fee;

            let (buyer_fee, seller_fee) = match fill.taker_side {
                Side::Buy => (taker_fee, maker_fee),
                Side::Sell => (maker_fee, taker_fee),
            };

            let maker_remaining = self
                .orderbook
                .get_order(fill.maker_order_id)
                .map(|o| o.remaining_qty)
                .unwrap_or(Decimal128::ZERO);

            events.push(Event::OrderFilled {
                order_id: fill.maker_order_id,
                trade_id,
                user_id: fill.maker_user_id,
                symbol,
                side: fill.taker_side.opposite(),
                fill_price: fill.price,
                fill_qty: fill.quantity,
                is_maker: true,
                remaining_qty: maker_remaining,
                timestamp,
            });

            events.push(Event::OrderFilled {
                order_id: fill.taker_order_id,
                trade_id,
                user_id: fill.taker_user_id,
                symbol,
                side: fill.taker_side,
                fill_price: fill.price,
                fill_qty: fill.quantity,
                is_maker: false,
                remaining_qty: taker.remaining_qty,
                timestamp,
            });

            events.push(Event::TradeExecuted {
                trade_id,
                symbol,
                price: fill.price,
                qty: fill.quantity,
                buyer_order_id,
                seller_order_id,
                buyer_user_id,
                seller_user_id,
                buyer_fee,
                seller_fee,
                timestamp,
            });
        }
    }

    /// Take a snapshot of the engine state.
    pub fn take_snapshot(&self) -> EngineSnapshot {
        let orders: Vec<BookOrder> = self.orderbook.all_orders().cloned().collect();
        let expiry_entries: Vec<(u64, OrderId)> = self
            .expiry_heap
            .iter()
            .map(|Reverse((t, id))| (t.as_micros(), *id))
            .collect();

        EngineSnapshot {
            symbol: self.orderbook.symbol,
            orders,
            stop_orders: self.stop_orders.clone(),
            mark_price: self.mark_price,
            index_price: self.index_price,
            sequence: self.sequence,
            trade_id_counter: 0, // SnowflakeGen state not easily extractable
            expiry_entries,
        }
    }

    /// Restore engine from a snapshot.
    pub fn restore_from_snapshot(
        snapshot: EngineSnapshot,
        config: SymbolConfig,
        node_id: u16,
    ) -> Self {
        let mut engine = Self::new(config, node_id);
        engine.mark_price = snapshot.mark_price;
        engine.index_price = snapshot.index_price;
        engine.sequence = snapshot.sequence;
        engine.stop_orders = snapshot.stop_orders;

        for order in snapshot.orders {
            engine.orderbook.insert_order(order);
        }

        for (micros, order_id) in snapshot.expiry_entries {
            engine
                .expiry_heap
                .push(Reverse((UnixMicros::from_micros(micros), order_id)));
        }

        engine
    }

    /// Accessor for the order book (for testing/inspection).
    pub fn orderbook(&self) -> &OrderBook {
        &self.orderbook
    }

    /// Current mark price.
    pub fn mark_price(&self) -> Decimal128 {
        self.mark_price
    }

    /// Set the mark price used by pre-trade risk checks. Stage 0 injects this
    /// once at boot from config; Stage 2+ replaces this with a real feed.
    pub fn set_mark_price(&mut self, price: Decimal128) {
        self.mark_price = price;
    }

    /// Current index price.
    pub fn index_price(&self) -> Decimal128 {
        self.index_price
    }

    /// Number of pending stop orders.
    pub fn stop_order_count(&self) -> usize {
        self.stop_orders.len()
    }

    /// Bind the current thread to a specific CPU core for low-latency operation.
    /// Call this from the matching engine's dedicated thread before entering the main loop.
    pub fn bind_to_core(core_id: usize) -> bool {
        let core_ids = core_affinity::get_core_ids().unwrap_or_default();
        if let Some(id) = core_ids.get(core_id) {
            core_affinity::set_for_current(*id)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn test_config() -> SymbolConfig {
        SymbolConfig {
            symbol: SymbolId::new(1),
            tick_size: dec("0.01"),
            lot_size: dec("0.001"),
            min_notional: dec("10"),
            max_leverage: dec("10"),
            maker_fee: dec("0.0002"),
            taker_fee: dec("0.0004"),
            margin_tiers: vec![],
        }
    }

    fn new_order_cmd(
        id: u64,
        user: u64,
        side: Side,
        order_type: OrderType,
        tif: TimeInForce,
        price: Option<&str>,
        qty: &str,
    ) -> Command {
        Command::NewOrder {
            order_id: OrderId::new(id),
            user_id: UserId::new(user),
            symbol: SymbolId::new(1),
            side,
            order_type,
            time_in_force: tif,
            price: price.map(dec),
            quantity: dec(qty),
            stop_price: None,
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: false,
            margin_mode: exg_common::MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id: None,
            timestamp: UnixMicros::from_micros(1_000_000 + id),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new_order_cmd_full(
        id: u64,
        user: u64,
        side: Side,
        order_type: OrderType,
        tif: TimeInForce,
        price: Option<&str>,
        qty: &str,
        stop_price: Option<&str>,
        trailing_delta: Option<&str>,
        visible_quantity: Option<&str>,
    ) -> Command {
        Command::NewOrder {
            order_id: OrderId::new(id),
            user_id: UserId::new(user),
            symbol: SymbolId::new(1),
            side,
            order_type,
            time_in_force: tif,
            price: price.map(dec),
            quantity: dec(qty),
            stop_price: stop_price.map(dec),
            trailing_delta: trailing_delta.map(dec),
            visible_quantity: visible_quantity.map(dec),
            reduce_only: false,
            margin_mode: exg_common::MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id: None,
            timestamp: UnixMicros::from_micros(1_000_000 + id),
        }
    }

    fn is_accepted(evt: &Event) -> bool {
        matches!(evt, Event::OrderAccepted { .. })
    }

    fn is_rejected(evt: &Event) -> bool {
        matches!(evt, Event::OrderRejected { .. })
    }

    fn is_canceled(evt: &Event) -> bool {
        matches!(evt, Event::OrderCanceled { .. })
    }

    fn is_filled(evt: &Event) -> bool {
        matches!(evt, Event::OrderFilled { .. })
    }

    fn is_trade(evt: &Event) -> bool {
        matches!(evt, Event::TradeExecuted { .. })
    }

    fn reject_reason(evt: &Event) -> Option<&RejectReason> {
        match evt {
            Event::OrderRejected { reason, .. } => Some(reason),
            _ => None,
        }
    }

    // 18. NewOrder → OrderAccepted + fills
    #[test]
    fn test_new_order_accepted_with_fills() {
        let mut engine = MatchingEngine::new(test_config(), 1);

        // Place a resting sell
        let sell = new_order_cmd(1, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10");
        let events = engine.process_command(&sell);
        assert_eq!(events.len(), 1);
        assert!(is_accepted(&events[0]));

        // Place a buy that crosses
        let buy = new_order_cmd(2, 20, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "5");
        let events = engine.process_command(&buy);

        // Should have: OrderAccepted, OrderFilled(maker), OrderFilled(taker), TradeExecuted
        assert!(is_accepted(&events[0]));
        let fills: Vec<_> = events.iter().filter(|e| is_filled(e)).collect();
        assert_eq!(fills.len(), 2);
        let trades: Vec<_> = events.iter().filter(|e| is_trade(e)).collect();
        assert_eq!(trades.len(), 1);
    }

    // 19. NewOrder rejected (invalid qty)
    #[test]
    fn test_new_order_rejected_invalid() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "0");
        let events = engine.process_command(&cmd);
        assert_eq!(events.len(), 1);
        assert!(is_rejected(&events[0]));
        assert_eq!(reject_reason(&events[0]), Some(&RejectReason::InvalidOrder));
    }

    // 20. CancelOrder → OrderCanceled
    #[test]
    fn test_cancel_order() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10");
        engine.process_command(&cmd);

        let cancel = Command::CancelOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(10),
            symbol: SymbolId::new(1),
            timestamp: UnixMicros::from_micros(2_000_000),
        };
        let events = engine.process_command(&cancel);
        assert_eq!(events.len(), 1);
        assert!(is_canceled(&events[0]));
    }

    // 21. CancelOrder on unknown order → rejected
    #[test]
    fn test_cancel_unknown_order() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cancel = Command::CancelOrder {
            order_id: OrderId::new(999),
            user_id: UserId::new(10),
            symbol: SymbolId::new(1),
            timestamp: UnixMicros::from_micros(2_000_000),
        };
        let events = engine.process_command(&cancel);
        assert_eq!(events.len(), 1);
        assert!(is_rejected(&events[0]));
        assert_eq!(
            reject_reason(&events[0]),
            Some(&RejectReason::OrderNotFound)
        );
    }

    // 22. AmendOrder price change → cancel + re-insert
    #[test]
    fn test_amend_order_price_change() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("49000"), "10");
        engine.process_command(&cmd);

        let amend = Command::AmendOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(10),
            symbol: SymbolId::new(1),
            new_price: Some(dec("50000")),
            new_quantity: None,
            timestamp: UnixMicros::from_micros(2_000_000),
        };
        let events = engine.process_command(&amend);

        // Should have cancel + accept
        assert!(events.iter().any(is_canceled));
        assert!(events.iter().any(is_accepted));

        // Verify order on book at new price
        assert_eq!(engine.orderbook().best_bid(), Some(dec("50000")));
    }

    // 23. AmendOrder qty down → in-place modify
    #[test]
    fn test_amend_order_qty_down() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10");
        engine.process_command(&cmd);

        let amend = Command::AmendOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(10),
            symbol: SymbolId::new(1),
            new_price: None,
            new_quantity: Some(dec("5")),
            timestamp: UnixMicros::from_micros(2_000_000),
        };
        let events = engine.process_command(&amend);

        // Should have accept (no cancel since qty down is in-place)
        assert_eq!(events.len(), 1);
        assert!(is_accepted(&events[0]));

        // Verify qty changed
        let order = engine.orderbook().get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.remaining_qty, dec("5"));
    }

    // 24. CancelAllOrders → multiple cancels
    #[test]
    fn test_cancel_all_orders() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        engine.process_command(&new_order_cmd(
            1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("49000"), "10",
        ));
        engine.process_command(&new_order_cmd(
            2, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("51000"), "20",
        ));
        engine.process_command(&new_order_cmd(
            3, 20, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("48000"), "5",
        ));

        let cancel_all = Command::CancelAllOrders {
            user_id: UserId::new(10),
            symbol: SymbolId::new(1),
            timestamp: UnixMicros::from_micros(2_000_000),
        };
        let events = engine.process_command(&cancel_all);

        let cancels: Vec<_> = events.iter().filter(|e| is_canceled(e)).collect();
        assert_eq!(cancels.len(), 2);

        // User 20's order still on book
        assert_eq!(engine.orderbook().order_count(), 1);
    }

    // 25. Stop order: set stop, update mark price past trigger → order activated and matched
    #[test]
    fn test_stop_order_trigger() {
        let mut engine = MatchingEngine::new(test_config(), 1);

        // Place a resting ask
        engine.process_command(&new_order_cmd(
            1, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("51000"), "10",
        ));

        // Place a stop-market buy at stop_price=51000
        let stop_buy = new_order_cmd_full(
            2, 20, Side::Buy, OrderType::StopMarket, TimeInForce::Gtc,
            None, "5", Some("51000"), None, None,
        );
        let events = engine.process_command(&stop_buy);
        assert!(is_accepted(&events[0]));
        assert_eq!(engine.stop_order_count(), 1);

        // Update mark price to trigger
        let events = engine.update_mark_price(dec("51000"), dec("51000"));

        // Stop should have triggered and matched
        assert!(!events.is_empty());
        let fills: Vec<_> = events.iter().filter(|e| is_filled(e)).collect();
        assert!(!fills.is_empty());
        assert_eq!(engine.stop_order_count(), 0);
    }

    // 26. Trailing stop: update mark price → peak tracked, reversal triggers
    #[test]
    fn test_trailing_stop() {
        let mut engine = MatchingEngine::new(test_config(), 1);

        // Set initial mark price
        engine.update_mark_price(dec("50000"), dec("50000"));

        // Place a resting bid for the triggered sell to match against
        engine.process_command(&new_order_cmd(
            1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("49000"), "100",
        ));

        // Place trailing stop sell with delta=1000
        let trailing = new_order_cmd_full(
            2, 20, Side::Sell, OrderType::TrailingStop, TimeInForce::Gtc,
            None, "5", None, Some("1000"), None,
        );
        let events = engine.process_command(&trailing);
        assert!(is_accepted(&events[0]));

        // Price goes up — peak should track
        let events = engine.update_mark_price(dec("52000"), dec("52000"));
        assert!(events.is_empty()); // no trigger yet (52000 - 1000 = 51000, mark=52000)

        // Price drops but not enough
        let events = engine.update_mark_price(dec("51500"), dec("51500"));
        assert!(events.is_empty()); // peak=52000, trigger at 51000, mark=51500

        // Price drops to trigger level
        let events = engine.update_mark_price(dec("51000"), dec("51000"));
        // Should trigger: peak=52000, delta=1000, trigger at 52000-1000=51000
        let fills: Vec<_> = events.iter().filter(|e| is_filled(e)).collect();
        assert!(!fills.is_empty());
        assert_eq!(engine.stop_order_count(), 0);
    }

    // 27. GTD expiration
    #[test]
    fn test_gtd_expiration() {
        let mut engine = MatchingEngine::new(test_config(), 1);

        // Create GTD order — expire_time is automatically set to timestamp + 24h
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtd, Some("50000"), "10");
        engine.process_command(&cmd);

        // The order timestamp is 1_000_001 micros, so expire = 1_000_001 + 86_400_000_000
        let expire_micros = 1_000_001u64 + 24 * 3600 * 1_000_000;

        // Check before expiry — nothing should happen
        let events = engine.check_expirations(UnixMicros::from_micros(expire_micros - 1));
        assert!(events.is_empty());

        // Check after expiry
        let events = engine.check_expirations(UnixMicros::from_micros(expire_micros + 1));
        assert_eq!(events.len(), 1);
        assert!(is_canceled(&events[0]));
    }

    // 28. Snapshot take + restore
    #[test]
    fn test_snapshot_restore() {
        let mut engine = MatchingEngine::new(test_config(), 1);

        // Place some orders
        engine.process_command(&new_order_cmd(
            1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("49000"), "10",
        ));
        engine.process_command(&new_order_cmd(
            2, 11, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("51000"), "20",
        ));

        // Place a stop order
        let stop = new_order_cmd_full(
            3, 12, Side::Buy, OrderType::StopMarket, TimeInForce::Gtc,
            None, "5", Some("52000"), None, None,
        );
        engine.process_command(&stop);

        engine.update_mark_price(dec("50000"), dec("50000"));

        let snapshot = engine.take_snapshot();

        // Restore
        let restored = MatchingEngine::restore_from_snapshot(snapshot, test_config(), 1);

        assert_eq!(restored.orderbook().order_count(), 2);
        assert_eq!(restored.stop_order_count(), 1);
        assert_eq!(restored.mark_price(), dec("50000"));
        assert_eq!(restored.orderbook().best_bid(), Some(dec("49000")));
        assert_eq!(restored.orderbook().best_ask(), Some(dec("51000")));
    }

    // Snapshot serde roundtrip
    #[test]
    fn test_snapshot_serde_roundtrip() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        engine.process_command(&new_order_cmd(
            1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("49000"), "10",
        ));

        let snapshot = engine.take_snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored_snapshot: EngineSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(restored_snapshot.symbol, SymbolId::new(1));
        assert_eq!(restored_snapshot.orders.len(), 1);
        assert_eq!(restored_snapshot.mark_price, Decimal128::ZERO);
    }

    // Duplicate order rejection
    #[test]
    fn test_duplicate_order_rejected() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10");
        engine.process_command(&cmd);

        let dup = new_order_cmd(1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10");
        let events = engine.process_command(&dup);
        assert!(is_rejected(&events[0]));
        assert_eq!(
            reject_reason(&events[0]),
            Some(&RejectReason::DuplicateOrder)
        );
    }

    // Post-only rejection via engine
    #[test]
    fn test_post_only_rejection_via_engine() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        engine.process_command(&new_order_cmd(
            1, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10",
        ));

        let post_only = new_order_cmd(
            2, 20, Side::Buy, OrderType::Limit, TimeInForce::PostOnly, Some("50000"), "5",
        );
        let events = engine.process_command(&post_only);
        assert_eq!(events.len(), 1);
        assert!(is_rejected(&events[0]));
        assert_eq!(
            reject_reason(&events[0]),
            Some(&RejectReason::PostOnlyWouldTake)
        );
    }

    // FOK rejection via engine
    #[test]
    fn test_fok_rejection_via_engine() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        engine.process_command(&new_order_cmd(
            1, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10",
        ));

        let fok = new_order_cmd(
            2, 20, Side::Buy, OrderType::Limit, TimeInForce::Fok, Some("50000"), "15",
        );
        let events = engine.process_command(&fok);
        assert_eq!(events.len(), 1);
        assert!(is_rejected(&events[0]));
        assert_eq!(
            reject_reason(&events[0]),
            Some(&RejectReason::FokNotFillable)
        );
        // Book should be unchanged
        assert_eq!(engine.orderbook().order_count(), 1);
    }

    // IOC partial fill with cancel via engine
    #[test]
    fn test_ioc_partial_fill_via_engine() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        engine.process_command(&new_order_cmd(
            1, 10, Side::Sell, OrderType::Limit, TimeInForce::Gtc, Some("50000"), "10",
        ));

        let ioc = new_order_cmd(
            2, 20, Side::Buy, OrderType::Limit, TimeInForce::Ioc, Some("50000"), "15",
        );
        let events = engine.process_command(&ioc);

        // Should have accept, fills, and a cancel for remaining 5
        assert!(events.iter().any(is_accepted));
        assert!(events.iter().any(is_filled));
        assert!(events.iter().any(is_canceled));

        if let Some(Event::OrderCanceled { remaining_qty, .. }) =
            events.iter().find(|e| is_canceled(e))
        {
            assert_eq!(*remaining_qty, dec("5"));
        }
    }

    // Market order with no depth
    #[test]
    fn test_market_order_no_depth() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let market = new_order_cmd(
            1, 10, Side::Buy, OrderType::Market, TimeInForce::Ioc, None, "10",
        );
        let events = engine.process_command(&market);
        // Accept + cancel (no depth)
        assert!(events.iter().any(is_accepted));
        assert!(events.iter().any(is_canceled));
    }

    // Limit order with no price → rejected
    #[test]
    fn test_limit_order_no_price() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        let cmd = new_order_cmd(
            1, 10, Side::Buy, OrderType::Limit, TimeInForce::Gtc, None, "10",
        );
        let events = engine.process_command(&cmd);
        assert!(is_rejected(&events[0]));
    }

    #[test]
    fn set_mark_price_updates_internal_state() {
        let mut engine = MatchingEngine::new(test_config(), 1);
        assert_eq!(engine.mark_price(), Decimal128::ZERO);
        engine.set_mark_price(dec("60000"));
        assert_eq!(engine.mark_price(), dec("60000"));
    }
}
