import { createContext } from "react"
import type { RootStore } from "./root-store"

export const StoreContext = createContext<RootStore | null>(null)
