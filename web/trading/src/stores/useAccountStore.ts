import { create } from "zustand";
import type { Position, Order, Balance } from "@/lib/types";
import { generatePositions, generateOrders, generateBalances } from "@/lib/mock-data";

interface AccountState {
  positions: Position[];
  activeOrders: Order[];
  orderHistory: Order[];
  balances: Balance[];
  init: () => void;
}

export const useAccountStore = create<AccountState>((set) => {
  const orders = generateOrders();
  return {
    positions: [],
    activeOrders: [],
    orderHistory: [],
    balances: [],
    init: () =>
      set({
        positions: generatePositions(),
        activeOrders: orders.active,
        orderHistory: orders.history,
        balances: generateBalances(),
      }),
  };
});
