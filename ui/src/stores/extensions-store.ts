import { makeAutoObservable, runInAction } from "mobx"

import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type { Extension, ExtensionCreate } from "./models"

export class ExtensionsStore {
  items: Extension[] = []
  isLoading = false
  isInstalling = false
  pendingRefreshNames = new Set<string>()
  error: string | null = null
  installError: string | null = null
  refreshErrors = new Map<string, string>()

  constructor(private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  get total(): number { return this.items.length }

  clearInstallError(): void { this.installError = null }
  isRefreshPending(name: string): boolean { return this.pendingRefreshNames.has(name) }
  refreshError(name: string): string | null { return this.refreshErrors.get(name) ?? null }
  clearRefreshError(name: string): void { this.refreshErrors.delete(name) }

  async loadExtensions(): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      const extensions = await this.api.get<Extension[]>("/extensions")
      runInAction(() => { this.items = extensions })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async installExtension(create: ExtensionCreate): Promise<Extension> {
    this.isInstalling = true
    this.installError = null
    try {
      const extension = await this.api.post<Extension>("/extensions", create)
      runInAction(() => { this.upsertExtension(extension) })
      return extension
    } catch (error) {
      runInAction(() => { this.installError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isInstalling = false })
    }
  }

  async refreshExtension(name: string): Promise<Extension> {
    this.pendingRefreshNames.add(name)
    this.refreshErrors.delete(name)
    try {
      const extension = await this.api.post<Extension>(`/extensions/${encodeURIComponent(name)}/refresh`)
      runInAction(() => { this.upsertExtension(extension) })
      return extension
    } catch (error) {
      runInAction(() => { this.refreshErrors.set(name, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingRefreshNames.delete(name) })
    }
  }

  private upsertExtension(extension: Extension): void {
    const exists = this.items.some((item) => item.name === extension.name)
    this.items = exists
      ? this.items.map((item) => item.name === extension.name ? extension : item)
      : [...this.items, extension]
  }
}
