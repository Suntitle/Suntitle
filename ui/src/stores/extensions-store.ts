/**
 * Registration persists desired state. Refresh requests preparation asynchronously.
 * Publication may be pending, preparing, published, or terminally failed. Resource
 * status is reported asynchronously by the worker.
 */
import { makeAutoObservable, runInAction } from "mobx"

import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type { Extension, ExtensionRegistration, ExtensionList } from "./models"

/** Owns registered resources and request state independently of worker publication. */
export class ExtensionsStore {
  /** Last successfully fetched registered resources. */
  items: Extension[] = []
  /** Whether a collection request is pending. */
  isLoading = false
  /** Whether registration is pending, independent of publication. */
  isInstalling = false
  /** Resource names with pending refresh HTTP requests. */
  pendingRefreshNames = new Set<string>()
  /** Most recent collection request failure. */
  error: string | null = null
  /** Most recent registration failure. */
  installError: string | null = null
  /** Refresh request failures by resource name. */
  refreshErrors = new Map<string, string>()

  /** Changes after successful mutations so older list responses cannot replace them. */
  private revision = 0

  /** Bind actions to the authenticated API client. */
  constructor(/** Transport used for resource requests. */ private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  /** Number of registered resources. */
  get total(): number { return this.items.length }

  /** Clear the registration failure before a new draft. */
  clearInstallError(): void { this.installError = null }
  /** Whether a resource refresh request is pending. */
  isRefreshPending(name: string): boolean { return this.pendingRefreshNames.has(name) }
  /** Last refresh failure for a resource. */
  refreshError(name: string): string | null { return this.refreshErrors.get(name) ?? null }
  /** Clear the failure for one refresh request. */
  clearRefreshError(name: string): void { this.refreshErrors.delete(name) }

  /** Read the resource list envelope without interpreting publication as installation. */
  async loadExtensions(): Promise<void> {
    if (this.isLoading) return
    const revision = this.revision
    this.isLoading = true
    this.error = null
    try {
      const extensions = await this.api.get<ExtensionList>("/extensions")
      runInAction(() => { if (revision === this.revision) this.items = extensions.items })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  /** Read the source manifest and register its desired state; publication remains deferred. */
  async installExtension(create: ExtensionRegistration): Promise<Extension> {
    this.isInstalling = true
    this.installError = null
    try {
      const extension = await this.api.post<Extension>("/extensions", create)
      runInAction(() => { this.revision++; this.upsertExtension(extension) })
      return extension
    } catch (error) {
      runInAction(() => { this.installError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isInstalling = false })
    }
  }

  /** Request asynchronous preparation using its URL identity. */
  async refreshExtension(name: string): Promise<Extension> {
    this.pendingRefreshNames.add(name)
    this.refreshErrors.delete(name)
    try {
      const extension = await this.api.post<Extension>(`/extensions/${encodeURIComponent(name)}/refresh`)
      runInAction(() => { this.revision++; this.upsertExtension(extension) })
      return extension
    } catch (error) {
      runInAction(() => { this.refreshErrors.set(name, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingRefreshNames.delete(name) })
    }
  }

  /** Replace the resource with matching server identity or append a new one. */
  private upsertExtension(extension: Extension): void {
    const exists = this.items.some((item) => item.name === extension.name)
    this.items = exists
      ? this.items.map((item) => item.name === extension.name ? extension : item)
      : [...this.items, extension]
  }
}
