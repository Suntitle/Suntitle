import type { PropsWithChildren } from "react"
import { RootStore } from "./root-store"
import { StoreContext } from "./store-context"

const defaultRootStore = new RootStore()

export function StoreProvider({ children, store = defaultRootStore }: PropsWithChildren<{ store?: RootStore }>) {
  return <StoreContext.Provider value={store}>{children}</StoreContext.Provider>
}
