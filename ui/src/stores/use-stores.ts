import { useContext } from "react"
import { StoreContext } from "./store-context"
import type { RootStore } from "./root-store"

export function useStores(): RootStore {
  const stores = useContext(StoreContext)
  if (!stores) throw new Error("useStores must be used within StoreProvider")
  return stores
}
