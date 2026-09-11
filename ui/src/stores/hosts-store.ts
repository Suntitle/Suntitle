import { mapHost } from "./resource-models"
import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type { Host, HostList, HostStatus } from "./models"

export class HostsStore {
  items: Host[] = []
  total = 0
  statusFilter: HostStatus | null = null
  isLoading = false
  error: string | null = null

  constructor(private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  get onlineHosts(): Host[] { return this.items.filter((host) => host.status === "online") }
  get hostById(): ReadonlyMap<string, Host> { return new Map(this.items.map((host) => [host.id, host])) }

  setStatusFilter(status: HostStatus | null): void { this.statusFilter = status }
  clearError(): void { this.error = null }

  async loadHosts(): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      const response = await this.api.get<HostList>("/hosts")
      runInAction(() => {
        this.items = response.items.map(mapHost).filter((host) => !this.statusFilter || host.status === this.statusFilter)
        this.total = this.items.length
      })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }
}
