import { ApiClient } from "./api-client"
import { AuthStore } from "./auth-store"
import { HostsStore } from "./hosts-store"
import { NodesStore } from "./nodes-store"

export class RootStore {
  readonly authStore: AuthStore
  readonly hostsStore: HostsStore
  readonly nodesStore: NodesStore

  constructor(baseUrl?: string) {
    let authStore: AuthStore
    const api = new ApiClient(() => authStore.accessToken, baseUrl)
    authStore = new AuthStore(api)

    this.authStore = authStore
    this.hostsStore = new HostsStore(api)
    this.nodesStore = new NodesStore(api)
  }
}
