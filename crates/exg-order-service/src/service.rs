use std::collections::VecDeque;

use exg_common::{
    Decimal128, MarginMode, OrderId, OrderStatus, OrderType, Side, SymbolId, TimeInForce, TradeId,
    UnixMicros, UserId,
};
use exg_protocol::{Command, Event};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::order::{FillRecord, Order};

/// Serializable snapshot of the entire order service state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderServiceSnapshot {
    pub orders: Vec<Order>,
    pub completed_orders: Vec<Order>,
    pub max_completed_history: usize,
}

/// In-memory order management service.
///
/// Consumes matching engine events and maintains order state with multiple
/// secondary indices for efficient lookup.
pub struct OrderService {
    /// Active orders indexed by order_id.
    orders: FxHashMap<OrderId, Order>,
    /// User's active orders: user_id -> [order_id].
    user_active_orders: FxHashMap<UserId, Vec<OrderId>>,
    /// Client order ID mapping: (user_id, client_order_id) -> order_id.
    client_order_map: FxHashMap<(UserId, u64), OrderId>,
    /// Recent completed orders (bounded ring buffer for history).
    completed_orders: VecDeque<Order>,
    max_completed_history: usize,
}

impl OrderService {
    pub fn new(max_completed_history: usize) -> Self {
        Self {
            orders: FxHashMap::default(),
            user_active_orders: FxHashMap::default(),
            client_order_map: FxHashMap::default(),
            completed_orders: VecDeque::with_capacity(max_completed_history),
            max_completed_history,
        }
    }

    /// Process an event from the matching engine, updating order state.
    pub fn apply_event(&mut self, event: &Event) {
        match event {
            Event::OrderAccepted {
                order_id,
                user_id,
                symbol,
                client_order_id,
                timestamp,
            } => {
                self.handle_order_accepted(
                    *order_id,
                    *user_id,
                    *symbol,
                    *client_order_id,
                    *timestamp,
                );
            }
            Event::OrderRejected {
                order_id,
                user_id,
                reason: _,
                timestamp,
            } => {
                self.handle_order_rejected(*order_id, *user_id, *timestamp);
            }
            Event::OrderFilled {
                order_id,
                trade_id,
                user_id: _,
                symbol: _,
                side: _,
                fill_price,
                fill_qty,
                is_maker,
                remaining_qty,
                timestamp,
            } => {
                self.handle_order_filled(
                    *order_id,
                    *trade_id,
                    *fill_price,
                    *fill_qty,
                    *is_maker,
                    *remaining_qty,
                    *timestamp,
                );
            }
            Event::OrderCanceled {
                order_id,
                user_id: _,
                symbol: _,
                remaining_qty: _,
                timestamp,
            } => {
                self.handle_order_canceled(*order_id, *timestamp);
            }
            // Other events are not relevant to order management.
            _ => {}
        }
    }

    /// Create a new order record from a NewOrder command.
    ///
    /// Returns `None` if the command is not a `NewOrder` variant.
    pub fn create_order(&mut self, cmd: &Command) -> Option<&Order> {
        let Command::NewOrder {
            order_id,
            user_id,
            symbol,
            side,
            order_type,
            time_in_force,
            price,
            quantity,
            stop_price,
            trailing_delta: _,
            visible_quantity: _,
            reduce_only,
            margin_mode,
            leverage,
            client_order_id,
            timestamp,
        } = cmd
        else {
            return None;
        };

        // Idempotency: if order already exists, return existing.
        if self.orders.contains_key(order_id) {
            return self.orders.get(order_id);
        }

        let order = Order {
            order_id: *order_id,
            user_id: *user_id,
            symbol: *symbol,
            client_order_id: *client_order_id,
            side: *side,
            order_type: *order_type,
            time_in_force: *time_in_force,
            price: *price,
            stop_price: *stop_price,
            original_qty: *quantity,
            executed_qty: Decimal128::ZERO,
            remaining_qty: *quantity,
            status: OrderStatus::New,
            margin_mode: *margin_mode,
            leverage: *leverage,
            reduce_only: *reduce_only,
            avg_fill_price: Decimal128::ZERO,
            total_filled_quote: Decimal128::ZERO,
            commission: Decimal128::ZERO,
            created_at: *timestamp,
            updated_at: *timestamp,
            fills: Vec::new(),
        };

        self.orders.insert(*order_id, order);
        self.user_active_orders
            .entry(*user_id)
            .or_default()
            .push(*order_id);
        if let Some(cid) = client_order_id {
            self.client_order_map.insert((*user_id, *cid), *order_id);
        }

        self.orders.get(order_id)
    }

    /// Create multiple orders from a batch of commands.
    ///
    /// Returns `Some(order_id)` for each successfully created NewOrder command,
    /// `None` for non-NewOrder commands or failures.
    pub fn batch_create_orders(&mut self, cmds: &[Command]) -> Vec<Option<OrderId>> {
        cmds.iter()
            .map(|cmd| self.create_order(cmd).map(|o| o.order_id))
            .collect()
    }

    /// Cancel all orders for a user on a symbol.
    ///
    /// Returns the order IDs that were canceled.
    pub fn cancel_user_symbol_orders(&mut self, user_id: UserId, symbol: SymbolId) -> Vec<OrderId> {
        let order_ids: Vec<OrderId> = self
            .user_active_orders
            .get(&user_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|oid| self.orders.get(oid))
                    .filter(|o| o.symbol == symbol && !o.status.is_terminal())
                    .map(|o| o.order_id)
                    .collect()
            })
            .unwrap_or_default();

        let now = if let Some(oid) = order_ids.first() {
            self.orders
                .get(oid)
                .map(|o| o.updated_at)
                .unwrap_or(UnixMicros::from_micros(0))
        } else {
            return Vec::new();
        };

        for &oid in &order_ids {
            if let Some(order) = self.orders.get_mut(&oid)
                && !order.status.is_terminal()
            {
                order.status = OrderStatus::Canceled;
                order.updated_at = now;
            }
            self.move_to_completed(oid, user_id);
        }

        order_ids
    }

    /// Get order by ID (active or still indexed).
    pub fn get_order(&self, order_id: OrderId) -> Option<&Order> {
        self.orders.get(&order_id)
    }

    /// Get order by client order ID.
    pub fn get_order_by_client_id(&self, user_id: UserId, client_order_id: u64) -> Option<&Order> {
        self.client_order_map
            .get(&(user_id, client_order_id))
            .and_then(|oid| self.orders.get(oid))
    }

    /// Get all active orders for a user.
    pub fn get_user_active_orders(&self, user_id: UserId) -> Vec<&Order> {
        self.user_active_orders
            .get(&user_id)
            .map(|ids| ids.iter().filter_map(|oid| self.orders.get(oid)).collect())
            .unwrap_or_default()
    }

    /// Get all active orders for a user on a specific symbol.
    pub fn get_user_symbol_orders(&self, user_id: UserId, symbol: SymbolId) -> Vec<&Order> {
        self.user_active_orders
            .get(&user_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|oid| self.orders.get(oid))
                    .filter(|o| o.symbol == symbol)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get recent completed orders for a user.
    pub fn get_user_completed_orders(&self, user_id: UserId) -> Vec<&Order> {
        self.completed_orders
            .iter()
            .filter(|o| o.user_id == user_id)
            .collect()
    }

    /// Total number of active orders.
    pub fn active_order_count(&self) -> usize {
        self.orders.len()
    }

    /// Snapshot all state for serialization.
    pub fn take_snapshot(&self) -> OrderServiceSnapshot {
        OrderServiceSnapshot {
            orders: self.orders.values().cloned().collect(),
            completed_orders: self.completed_orders.iter().cloned().collect(),
            max_completed_history: self.max_completed_history,
        }
    }

    /// Restore from snapshot.
    pub fn restore_from_snapshot(snapshot: OrderServiceSnapshot) -> Self {
        let mut svc = Self::new(snapshot.max_completed_history);

        for order in snapshot.orders {
            // Rebuild secondary indices.
            svc.user_active_orders
                .entry(order.user_id)
                .or_default()
                .push(order.order_id);
            if let Some(cid) = order.client_order_id {
                svc.client_order_map
                    .insert((order.user_id, cid), order.order_id);
            }
            svc.orders.insert(order.order_id, order);
        }

        svc.completed_orders = snapshot.completed_orders.into();
        svc
    }

    // ── Private event handlers ─────────────────────────────────────────

    fn handle_order_accepted(
        &mut self,
        order_id: OrderId,
        user_id: UserId,
        symbol: SymbolId,
        client_order_id: Option<u64>,
        timestamp: UnixMicros,
    ) {
        if let Some(order) = self.orders.get_mut(&order_id) {
            // Idempotency: already processed.
            if order.status != OrderStatus::New || !order.fills.is_empty() {
                return;
            }
            // If it's a conditional order, set to PendingTrigger.
            if order.order_type.is_conditional() {
                order.status = OrderStatus::PendingTrigger;
            }
            order.updated_at = timestamp;
            return;
        }

        // Order not yet created via create_order — create a minimal record.
        // This handles the case where apply_event is called directly without
        // a preceding create_order (e.g., during replay).
        let status = if self.is_conditional_order_type_from_context() {
            OrderStatus::PendingTrigger
        } else {
            OrderStatus::New
        };

        let order = Order {
            order_id,
            user_id,
            symbol,
            client_order_id,
            side: Side::Buy, // Unknown from event alone — placeholder.
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            price: None,
            stop_price: None,
            original_qty: Decimal128::ZERO,
            executed_qty: Decimal128::ZERO,
            remaining_qty: Decimal128::ZERO,
            status,
            margin_mode: MarginMode::Cross,
            leverage: None,
            reduce_only: false,
            avg_fill_price: Decimal128::ZERO,
            total_filled_quote: Decimal128::ZERO,
            commission: Decimal128::ZERO,
            created_at: timestamp,
            updated_at: timestamp,
            fills: Vec::new(),
        };

        self.orders.insert(order_id, order);
        self.user_active_orders
            .entry(user_id)
            .or_default()
            .push(order_id);
        if let Some(cid) = client_order_id {
            self.client_order_map.insert((user_id, cid), order_id);
        }
    }

    fn handle_order_rejected(&mut self, order_id: OrderId, user_id: UserId, timestamp: UnixMicros) {
        if let Some(order) = self.orders.get_mut(&order_id) {
            // Idempotency: already terminal.
            if order.status.is_terminal() {
                return;
            }
            order.status = OrderStatus::Rejected;
            order.updated_at = timestamp;
            self.move_to_completed(order_id, user_id);
        } else {
            // Idempotency: check if already in completed orders.
            if self.completed_orders.iter().any(|o| o.order_id == order_id) {
                return;
            }

            // Create a minimal rejected order record.
            let order = Order {
                order_id,
                user_id,
                symbol: SymbolId::new(0),
                client_order_id: None,
                side: Side::Buy,
                order_type: OrderType::Limit,
                time_in_force: TimeInForce::Gtc,
                price: None,
                stop_price: None,
                original_qty: Decimal128::ZERO,
                executed_qty: Decimal128::ZERO,
                remaining_qty: Decimal128::ZERO,
                status: OrderStatus::Rejected,
                margin_mode: MarginMode::Cross,
                leverage: None,
                reduce_only: false,
                avg_fill_price: Decimal128::ZERO,
                total_filled_quote: Decimal128::ZERO,
                commission: Decimal128::ZERO,
                created_at: timestamp,
                updated_at: timestamp,
                fills: Vec::new(),
            };
            self.push_completed(order);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_order_filled(
        &mut self,
        order_id: OrderId,
        trade_id: TradeId,
        fill_price: Decimal128,
        fill_qty: Decimal128,
        is_maker: bool,
        remaining_qty: Decimal128,
        timestamp: UnixMicros,
    ) {
        let Some(order) = self.orders.get_mut(&order_id) else {
            return;
        };

        // Idempotency: check if this trade_id was already applied.
        if order.fills.iter().any(|f| f.trade_id == trade_id) {
            return;
        }

        // Idempotency: already in terminal state.
        if order.status.is_terminal() {
            return;
        }

        let fill_record = FillRecord {
            trade_id,
            price: fill_price,
            qty: fill_qty,
            is_maker,
            commission: Decimal128::ZERO, // Commission set by TradeExecuted or externally.
            timestamp,
        };

        order.fills.push(fill_record);
        order.executed_qty = order.executed_qty + fill_qty;
        order.remaining_qty = remaining_qty;
        order.total_filled_quote = order.total_filled_quote + (fill_price * fill_qty);

        // Recompute VWAP.
        if !order.executed_qty.is_zero() {
            order.avg_fill_price = order.total_filled_quote / order.executed_qty;
        }

        order.updated_at = timestamp;

        if remaining_qty.is_zero() {
            order.status = OrderStatus::Filled;
            let user_id = order.user_id;
            self.move_to_completed(order_id, user_id);
        } else {
            order.status = OrderStatus::PartiallyFilled;
        }
    }

    fn handle_order_canceled(&mut self, order_id: OrderId, timestamp: UnixMicros) {
        let Some(order) = self.orders.get_mut(&order_id) else {
            return;
        };

        // Idempotency: already terminal.
        if order.status.is_terminal() {
            return;
        }

        order.status = OrderStatus::Canceled;
        order.updated_at = timestamp;
        let user_id = order.user_id;
        self.move_to_completed(order_id, user_id);
    }

    /// Move an order from active to completed, updating all indices.
    fn move_to_completed(&mut self, order_id: OrderId, user_id: UserId) {
        if let Some(order) = self.orders.remove(&order_id) {
            // Remove from user active orders.
            if let Some(ids) = self.user_active_orders.get_mut(&user_id) {
                ids.retain(|id| *id != order_id);
                if ids.is_empty() {
                    self.user_active_orders.remove(&user_id);
                }
            }
            // Remove from client order map.
            if let Some(cid) = order.client_order_id {
                self.client_order_map.remove(&(user_id, cid));
            }
            self.push_completed(order);
        }
    }

    /// Push an order into the completed ring buffer, evicting oldest if at capacity.
    fn push_completed(&mut self, order: Order) {
        if self.completed_orders.len() >= self.max_completed_history {
            self.completed_orders.pop_front();
        }
        self.completed_orders.push_back(order);
    }

    /// Placeholder — in production this would check the order_type from the
    /// original command. Since OrderAccepted doesn't carry order_type, we
    /// default to non-conditional when creating from event alone.
    fn is_conditional_order_type_from_context(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exg_common::TradeId;

    fn dec(s: &str) -> Decimal128 {
        s.parse().unwrap()
    }

    fn ts(us: u64) -> UnixMicros {
        UnixMicros::from_micros(us)
    }

    fn make_new_order_cmd(
        order_id: u64,
        user_id: u64,
        symbol: u16,
        side: Side,
        order_type: OrderType,
        price: Option<&str>,
        qty: &str,
        client_order_id: Option<u64>,
    ) -> Command {
        Command::NewOrder {
            order_id: OrderId::new(order_id),
            user_id: UserId::new(user_id),
            symbol: SymbolId::new(symbol),
            side,
            order_type,
            time_in_force: TimeInForce::Gtc,
            price: price.map(dec),
            quantity: dec(qty),
            stop_price: None,
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: false,
            margin_mode: MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id,
            timestamp: ts(1_000_000),
        }
    }

    // ── Test 1: Create order from NewOrder command ──────────────────

    #[test]
    fn test_create_order_from_command() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.5",
            Some(9999),
        );
        let order = svc.create_order(&cmd).unwrap();

        assert_eq!(order.order_id, OrderId::new(1));
        assert_eq!(order.user_id, UserId::new(42));
        assert_eq!(order.symbol, SymbolId::new(1));
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.price, Some(dec("50000")));
        assert_eq!(order.original_qty, dec("1.5"));
        assert_eq!(order.remaining_qty, dec("1.5"));
        assert_eq!(order.executed_qty, Decimal128::ZERO);
        assert_eq!(order.status, OrderStatus::New);
        assert_eq!(order.client_order_id, Some(9999));
        assert!(order.fills.is_empty());
    }

    // ── Test 2: OrderAccepted → status = New ───────────────────────

    #[test]
    fn test_order_accepted_status_new() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.5",
            None,
        );
        svc.create_order(&cmd);

        let event = Event::OrderAccepted {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(2_000_000),
        };
        svc.apply_event(&event);

        let order = svc.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.status, OrderStatus::New);
    }

    // ── Test 2b: OrderAccepted for conditional → PendingTrigger ────

    #[test]
    fn test_order_accepted_conditional_pending_trigger() {
        let mut svc = OrderService::new(100);
        let cmd = Command::NewOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            order_type: OrderType::StopLimit,
            time_in_force: TimeInForce::Gtc,
            price: Some(dec("49000")),
            quantity: dec("2"),
            stop_price: Some(dec("49500")),
            trailing_delta: None,
            visible_quantity: None,
            reduce_only: false,
            margin_mode: MarginMode::Cross,
            leverage: Some(dec("10")),
            client_order_id: None,
            timestamp: ts(1_000_000),
        };
        svc.create_order(&cmd);

        let event = Event::OrderAccepted {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(2_000_000),
        };
        svc.apply_event(&event);

        let order = svc.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.status, OrderStatus::PendingTrigger);
    }

    // ── Test 3: Partial fill → PartiallyFilled ─────────────────────

    #[test]
    fn test_partial_fill() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "2.0",
            None,
        );
        svc.create_order(&cmd);

        let fill_event = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(100),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("50000"),
            fill_qty: dec("0.5"),
            is_maker: true,
            remaining_qty: dec("1.5"),
            timestamp: ts(3_000_000),
        };
        svc.apply_event(&fill_event);

        let order = svc.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.status, OrderStatus::PartiallyFilled);
        assert_eq!(order.executed_qty, dec("0.5"));
        assert_eq!(order.remaining_qty, dec("1.5"));
        assert_eq!(order.fills.len(), 1);
        // Still active.
        assert_eq!(svc.active_order_count(), 1);
    }

    // ── Test 4: Full fill → Filled, moved to completed ─────────────

    #[test]
    fn test_full_fill_moved_to_completed() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        );
        svc.create_order(&cmd);

        let fill_event = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(100),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("50000"),
            fill_qty: dec("1.0"),
            is_maker: true,
            remaining_qty: Decimal128::ZERO,
            timestamp: ts(3_000_000),
        };
        svc.apply_event(&fill_event);

        // No longer in active orders.
        assert!(svc.get_order(OrderId::new(1)).is_none());
        assert_eq!(svc.active_order_count(), 0);

        // In completed orders.
        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].status, OrderStatus::Filled);
        assert_eq!(completed[0].executed_qty, dec("1.0"));
    }

    // ── Test 5: OrderCanceled → moved to completed ─────────────────

    #[test]
    fn test_order_canceled() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        );
        svc.create_order(&cmd);

        let cancel_event = Event::OrderCanceled {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            remaining_qty: dec("1.0"),
            timestamp: ts(4_000_000),
        };
        svc.apply_event(&cancel_event);

        assert!(svc.get_order(OrderId::new(1)).is_none());
        assert_eq!(svc.active_order_count(), 0);

        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].status, OrderStatus::Canceled);
    }

    // ── Test 6: OrderRejected → immediately in completed ───────────

    #[test]
    fn test_order_rejected() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        );
        svc.create_order(&cmd);

        let reject_event = Event::OrderRejected {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            reason: exg_protocol::RejectReason::InsufficientMargin,
            timestamp: ts(2_000_000),
        };
        svc.apply_event(&reject_event);

        assert!(svc.get_order(OrderId::new(1)).is_none());
        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].status, OrderStatus::Rejected);
    }

    // ── Test 6b: OrderRejected without prior create_order ──────────

    #[test]
    fn test_order_rejected_without_create() {
        let mut svc = OrderService::new(100);
        let reject_event = Event::OrderRejected {
            order_id: OrderId::new(99),
            user_id: UserId::new(42),
            reason: exg_protocol::RejectReason::InvalidOrder,
            timestamp: ts(2_000_000),
        };
        svc.apply_event(&reject_event);

        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].status, OrderStatus::Rejected);
    }

    // ── Test 7: VWAP correctness with two fills at different prices ─

    #[test]
    fn test_avg_fill_price_vwap() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("51000"),
            "3.0",
            None,
        );
        svc.create_order(&cmd);

        // Fill 1: 1.0 @ 50000
        let fill1 = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(100),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("50000"),
            fill_qty: dec("1.0"),
            is_maker: true,
            remaining_qty: dec("2.0"),
            timestamp: ts(3_000_000),
        };
        svc.apply_event(&fill1);

        // Fill 2: 2.0 @ 51000
        let fill2 = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(101),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("51000"),
            fill_qty: dec("2.0"),
            is_maker: false,
            remaining_qty: Decimal128::ZERO,
            timestamp: ts(4_000_000),
        };
        svc.apply_event(&fill2);

        // VWAP = (1*50000 + 2*51000) / 3 = 152000 / 3 = 50666.666...
        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        let order = &completed[0];
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(order.executed_qty, dec("3.0"));
        assert_eq!(order.total_filled_quote, dec("152000"));

        // VWAP: 152000 / 3 — check within 1 ULP tolerance.
        let expected_vwap = dec("152000") / dec("3");
        let diff = (order.avg_fill_price - expected_vwap).abs();
        assert!(diff.raw() <= 1, "VWAP diff too large: {diff}");
    }

    // ── Test 8: Client order ID lookup ─────────────────────────────

    #[test]
    fn test_client_order_id_lookup() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            Some(12345),
        );
        svc.create_order(&cmd);

        let order = svc.get_order_by_client_id(UserId::new(42), 12345).unwrap();
        assert_eq!(order.order_id, OrderId::new(1));

        // Non-existent client ID.
        assert!(svc.get_order_by_client_id(UserId::new(42), 99999).is_none());
        // Wrong user.
        assert!(svc.get_order_by_client_id(UserId::new(99), 12345).is_none());
    }

    // ── Test 9: User active orders query ───────────────────────────

    #[test]
    fn test_user_active_orders() {
        let mut svc = OrderService::new(100);

        // User 42: two orders.
        svc.create_order(&make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        ));
        svc.create_order(&make_new_order_cmd(
            2,
            42,
            2,
            Side::Sell,
            OrderType::Market,
            None,
            "0.5",
            None,
        ));
        // User 43: one order.
        svc.create_order(&make_new_order_cmd(
            3,
            43,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("49000"),
            "2.0",
            None,
        ));

        let user42_orders = svc.get_user_active_orders(UserId::new(42));
        assert_eq!(user42_orders.len(), 2);

        let user43_orders = svc.get_user_active_orders(UserId::new(43));
        assert_eq!(user43_orders.len(), 1);

        let user99_orders = svc.get_user_active_orders(UserId::new(99));
        assert!(user99_orders.is_empty());
    }

    // ── Test 10: User symbol orders filter ─────────────────────────

    #[test]
    fn test_user_symbol_orders() {
        let mut svc = OrderService::new(100);

        svc.create_order(&make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        ));
        svc.create_order(&make_new_order_cmd(
            2,
            42,
            2,
            Side::Sell,
            OrderType::Limit,
            Some("3000"),
            "5.0",
            None,
        ));
        svc.create_order(&make_new_order_cmd(
            3,
            42,
            1,
            Side::Sell,
            OrderType::Limit,
            Some("51000"),
            "0.5",
            None,
        ));

        let sym1_orders = svc.get_user_symbol_orders(UserId::new(42), SymbolId::new(1));
        assert_eq!(sym1_orders.len(), 2);

        let sym2_orders = svc.get_user_symbol_orders(UserId::new(42), SymbolId::new(2));
        assert_eq!(sym2_orders.len(), 1);

        let sym3_orders = svc.get_user_symbol_orders(UserId::new(42), SymbolId::new(3));
        assert!(sym3_orders.is_empty());
    }

    // ── Test 11: Completed orders bounded by max_history ───────────

    #[test]
    fn test_completed_orders_bounded() {
        let mut svc = OrderService::new(3); // Max 3 completed.

        for i in 1..=5u64 {
            svc.create_order(&make_new_order_cmd(
                i,
                42,
                1,
                Side::Buy,
                OrderType::Limit,
                Some("50000"),
                "1.0",
                None,
            ));
            let cancel = Event::OrderCanceled {
                order_id: OrderId::new(i),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                remaining_qty: dec("1.0"),
                timestamp: ts(i * 1_000_000),
            };
            svc.apply_event(&cancel);
        }

        // Should only keep the last 3.
        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 3);
        // Oldest remaining should be order 3.
        assert_eq!(completed[0].order_id, OrderId::new(3));
        assert_eq!(completed[1].order_id, OrderId::new(4));
        assert_eq!(completed[2].order_id, OrderId::new(5));
    }

    // ── Test 12: Snapshot/restore roundtrip ─────────────────────────

    #[test]
    fn test_snapshot_restore_roundtrip() {
        let mut svc = OrderService::new(100);

        svc.create_order(&make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "2.0",
            Some(111),
        ));

        // Partial fill.
        let fill = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(100),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("50000"),
            fill_qty: dec("1.0"),
            is_maker: true,
            remaining_qty: dec("1.0"),
            timestamp: ts(3_000_000),
        };
        svc.apply_event(&fill);

        // Add a second order and cancel it.
        svc.create_order(&make_new_order_cmd(
            2,
            42,
            1,
            Side::Sell,
            OrderType::Limit,
            Some("51000"),
            "0.5",
            None,
        ));
        let cancel = Event::OrderCanceled {
            order_id: OrderId::new(2),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            remaining_qty: dec("0.5"),
            timestamp: ts(4_000_000),
        };
        svc.apply_event(&cancel);

        // Take snapshot and serialize.
        let snapshot = svc.take_snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored_snapshot: OrderServiceSnapshot = serde_json::from_str(&json).unwrap();
        let restored = OrderService::restore_from_snapshot(restored_snapshot);

        // Verify active orders.
        assert_eq!(restored.active_order_count(), 1);
        let order = restored.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.status, OrderStatus::PartiallyFilled);
        assert_eq!(order.executed_qty, dec("1.0"));
        assert_eq!(order.remaining_qty, dec("1.0"));
        assert_eq!(order.fills.len(), 1);

        // Verify client order ID lookup still works.
        assert!(
            restored
                .get_order_by_client_id(UserId::new(42), 111)
                .is_some()
        );

        // Verify completed orders.
        let completed = restored.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].order_id, OrderId::new(2));
        assert_eq!(completed[0].status, OrderStatus::Canceled);
    }

    // ── Test 13: Idempotent event application ──────────────────────

    #[test]
    fn test_idempotent_fill_event() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "2.0",
            None,
        );
        svc.create_order(&cmd);

        let fill = Event::OrderFilled {
            order_id: OrderId::new(1),
            trade_id: TradeId::new(100),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            side: Side::Buy,
            fill_price: dec("50000"),
            fill_qty: dec("1.0"),
            is_maker: true,
            remaining_qty: dec("1.0"),
            timestamp: ts(3_000_000),
        };

        // Apply same event twice.
        svc.apply_event(&fill);
        svc.apply_event(&fill);

        let order = svc.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.executed_qty, dec("1.0")); // Not 2.0.
        assert_eq!(order.fills.len(), 1); // Not 2.
    }

    #[test]
    fn test_idempotent_cancel_event() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        );
        svc.create_order(&cmd);

        let cancel = Event::OrderCanceled {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            remaining_qty: dec("1.0"),
            timestamp: ts(4_000_000),
        };

        svc.apply_event(&cancel);
        svc.apply_event(&cancel); // Second apply is a no-op.

        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
    }

    #[test]
    fn test_idempotent_reject_event() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        );
        svc.create_order(&cmd);

        let reject = Event::OrderRejected {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            reason: exg_protocol::RejectReason::InsufficientMargin,
            timestamp: ts(2_000_000),
        };

        svc.apply_event(&reject);
        svc.apply_event(&reject); // Second apply is a no-op.

        let completed = svc.get_user_completed_orders(UserId::new(42));
        assert_eq!(completed.len(), 1);
    }

    #[test]
    fn test_idempotent_create_order() {
        let mut svc = OrderService::new(100);
        let cmd = make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            Some(555),
        );

        svc.create_order(&cmd);
        svc.create_order(&cmd); // Idempotent.

        assert_eq!(svc.active_order_count(), 1);
        let user_orders = svc.get_user_active_orders(UserId::new(42));
        assert_eq!(user_orders.len(), 1);
    }

    // ── Non-NewOrder command returns None ───────────────────────────

    #[test]
    fn test_create_order_non_new_order_returns_none() {
        let mut svc = OrderService::new(100);
        let cmd = Command::CancelOrder {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            timestamp: ts(1_000_000),
        };
        assert!(svc.create_order(&cmd).is_none());
    }

    // ── Ignored events don't affect state ──────────────────────────

    #[test]
    fn test_ignored_events() {
        let mut svc = OrderService::new(100);
        svc.create_order(&make_new_order_cmd(
            1,
            42,
            1,
            Side::Buy,
            OrderType::Limit,
            Some("50000"),
            "1.0",
            None,
        ));

        let mark_event = Event::MarkPriceUpdate {
            symbol: SymbolId::new(1),
            mark_price: dec("50001"),
            index_price: dec("50000"),
            timestamp: ts(5_000_000),
        };
        svc.apply_event(&mark_event);

        let funding_event = Event::FundingRateUpdate {
            symbol: SymbolId::new(1),
            funding_rate: dec("0.0001"),
            timestamp: ts(5_000_000),
        };
        svc.apply_event(&funding_event);

        // State unchanged.
        assert_eq!(svc.active_order_count(), 1);
        let order = svc.get_order(OrderId::new(1)).unwrap();
        assert_eq!(order.status, OrderStatus::New);
    }

    // ── Test: batch_create_orders ─────────────────────────────────────

    #[test]
    fn test_batch_create_orders() {
        let mut svc = OrderService::new(100);

        let cmds = vec![
            make_new_order_cmd(
                1,
                42,
                1,
                Side::Buy,
                OrderType::Limit,
                Some("50000"),
                "1.0",
                None,
            ),
            make_new_order_cmd(
                2,
                42,
                1,
                Side::Sell,
                OrderType::Limit,
                Some("51000"),
                "2.0",
                None,
            ),
            // Non-NewOrder command — should return None.
            Command::CancelOrder {
                order_id: OrderId::new(99),
                user_id: UserId::new(42),
                symbol: SymbolId::new(1),
                timestamp: ts(1_000_000),
            },
            make_new_order_cmd(3, 43, 2, Side::Buy, OrderType::Market, None, "0.5", None),
        ];

        let results = svc.batch_create_orders(&cmds);

        assert_eq!(results.len(), 4);
        assert_eq!(results[0], Some(OrderId::new(1)));
        assert_eq!(results[1], Some(OrderId::new(2)));
        assert_eq!(results[2], None); // CancelOrder
        assert_eq!(results[3], Some(OrderId::new(3)));

        assert_eq!(svc.active_order_count(), 3);
    }
}
