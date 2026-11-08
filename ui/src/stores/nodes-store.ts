import type { ExtensionsStore } from "./extensions-store"
import { mapNode } from "./resource-models"
import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { getErrorMessage } from "./api-client"
import type {
  Node,
  NodeCreate,
  NodeResource,
  NodeList,
  HostList,
  HostResource,
  NodeStatus,
  NodeRole,
  NodeTypeDefinition,
  NodeTypeDefinitionList,
  NodeUpdate,
} from "./models"

export class NodesStore {
  private hosts: HostResource[] = []
  items: Node[] = []
  total = 0
  nodeTypes: NodeTypeDefinition[] = []
  nodeTypesTotal = 0
  selectedNode: Node | null = null
  typeFilter: NodeRole | null = null
  statusFilter: NodeStatus | null = null
  isLoading = false
  isLoadingNodeTypes = false
  isSaving = false
  isInstalling = false
  pendingNodeActionIds = new Set<string>()
  error: string | null = null
  nodeTypesError: string | null = null
  installError: string | null = null
  saveError: string | null = null
  nodeActionErrors = new Map<string, string>()

  /** Use the shared extension registry to resolve custom node roles. */
  constructor(/** Authenticated resource transport. */ private readonly api: ApiClient, /** Shared extension definitions for custom node roles. */ private readonly extensions: ExtensionsStore) {
    makeAutoObservable<this, "api" | "extensions">(this, { api: false, extensions: false }, { autoBind: true })
  }

  get enabledNodes(): Node[] { return this.items.filter((node) => node.enabled) }
  get nodeById(): ReadonlyMap<string, Node> { return new Map(this.items.map((node) => [node.id, node])) }

  nodesForHost(hostId: string): Node[] {
    return this.items.filter((node) => node.host.id === hostId)
  }

  get nodesByType(): ReadonlyMap<NodeRole, Node[]> {
    const groups = new Map<NodeRole, Node[]>([
      ["audio_source", []], ["transcriber", []], ["translator", []], ["display", []], ["custom", []],
    ])
    this.items.forEach((node) => groups.get(node.type)?.push(node))
    return groups
  }

  setTypeFilter(type: NodeRole | null): void { this.typeFilter = type }
  setStatusFilter(status: NodeStatus | null): void { this.statusFilter = status }
  selectNode(node: Node | null): void { this.selectedNode = node }
  clearError(): void { this.error = null }
  clearInstallError(): void { this.installError = null }
  /** Expose form validation failures through the same action as API failures. */
  setInstallError(error: unknown): void { this.installError = getErrorMessage(error) }
  clearSaveError(): void { this.saveError = null }
  /** Keep custom-setting validation errors inside an observable action. */
  setSaveError(error: unknown): void { this.saveError = getErrorMessage(error) }
  isNodeActionPending(nodeId: string): boolean { return this.pendingNodeActionIds.has(nodeId) }
  nodeActionError(nodeId: string): string | null { return this.nodeActionErrors.get(nodeId) ?? null }
  clearNodeActionError(nodeId: string): void { this.nodeActionErrors.delete(nodeId) }

  async loadNodeTypes(): Promise<void> {
    this.isLoadingNodeTypes = true
    this.nodeTypesError = null
    try {
      const response = await this.api.get<NodeTypeDefinitionList>("/node-types")
      runInAction(() => {
        this.nodeTypes = response.items
        this.nodeTypesTotal = response.total
      })
    } catch (error) {
      runInAction(() => { this.nodeTypesError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoadingNodeTypes = false })
    }
  }

  /** Load resources and resolve extension-backed roles before mapping node models. */
  async loadNodes(): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      const [response, hosts] = await Promise.all([
        this.api.get<NodeList>("/nodes"),
        this.api.get<HostList>("/hosts"),
      ])
      if (response.items.some((resource) => resource.extensionRef)) await this.extensions.loadExtensions().catch(() => undefined)
      runInAction(() => {
        this.hosts = hosts.items
        this.items = response.items.map((resource) => mapNode(resource, this.hosts, this.extensions.items))
          .filter((node) => (!this.typeFilter || node.type === this.typeFilter) && (!this.statusFilter || node.status === this.statusFilter))
        this.total = this.items.length
      })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async loadNode(nodeId: string): Promise<Node> {
    this.isLoading = true
    this.error = null
    try {
      const node = mapNode(await this.api.get<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}`), this.hosts, this.extensions.items)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async updateNode(nodeId: string, update: NodeUpdate): Promise<Node> {
    this.isSaving = true
    this.saveError = null
    try {
      const node = mapNode(await this.api.put<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}`, this.specUpdate(nodeId, update)), this.hosts, this.extensions.items)
      runInAction(() => {
        this.selectedNode = node
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.saveError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isSaving = false })
    }
  }

  /** Persist a built-in variant or custom settings with its extension reference. */
  async installNode(hostId: string, create: NodeCreate): Promise<Node> {
    this.isInstalling = true
    this.installError = null
    try {
      const host = this.hosts.find((host) => host.name === hostId)
      if (!host) throw new Error("Host is no longer available")
      const { inputNodeId, type, settings, extensionRef, name, ...fields } = create
      const node = mapNode(await this.api.post<NodeResource>(`/hosts/${encodeURIComponent(hostId)}/nodes`, {
        ...fields, displayName: name, hostRef: { name: host.name },
        inputRef: inputNodeId ? { name: inputNodeId } : null,
        ...(extensionRef ? { extensionRef: { name: extensionRef } } : {}), settings: { [extensionRef ? "custom" : type]: extensionRef ? settings : this.settingsForInput(type, settings, inputNodeId) },
      }), this.hosts, this.extensions.items)
      runInAction(() => {
        this.upsertNode(node)
      })
      return node
    } catch (error) {
      runInAction(() => { this.installError = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isInstalling = false })
    }
  }

  async restartNode(nodeId: string): Promise<Node> {
    this.pendingNodeActionIds.add(nodeId)
    this.nodeActionErrors.delete(nodeId)
    try {
      const node = mapNode(await this.api.post<NodeResource>(`/nodes/${encodeURIComponent(nodeId)}/restart`), this.hosts, this.extensions.items)
      runInAction(() => { this.upsertNode(node) })
      return node
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(nodeId, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(nodeId) })
    }
  }

  async disableNode(node: Node): Promise<Node> {
    this.pendingNodeActionIds.add(node.id)
    this.nodeActionErrors.delete(node.id)
    try {
      const updatedNode = mapNode(await this.api.put<NodeResource>(`/nodes/${encodeURIComponent(node.id)}`, { ...node.spec, enabled: false }), this.hosts, this.extensions.items)
      runInAction(() => { this.upsertNode(updatedNode) })
      return updatedNode
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(node.id, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(node.id) })
    }
  }

  async deleteNode(nodeId: string): Promise<void> {
    if (this.isNodeActionPending(nodeId)) return
    this.pendingNodeActionIds.add(nodeId)
    this.nodeActionErrors.delete(nodeId)
    try {
      await this.api.delete(`/nodes/${encodeURIComponent(nodeId)}`)
      await this.loadNodes()
      runInAction(() => {
        if (this.selectedNode?.id === nodeId) this.selectedNode = null
      })
    } catch (error) {
      runInAction(() => { this.nodeActionErrors.set(nodeId, getErrorMessage(error)) })
      throw error
    } finally {
      runInAction(() => { this.pendingNodeActionIds.delete(nodeId) })
    }
  }

  /** Preserve extension identity and serialize edits to the matching settings variant. */
  private specUpdate(nodeId: string, update: NodeUpdate) {
    const node = this.nodeById.get(nodeId)
    if (!node) throw new Error("Node is no longer available")
    const { inputNodeId, type, settings, name, ...fields } = update
    const { extensionRef: _extensionRef, ...editable } = fields as typeof fields & { extensionRef?: string }
    return { ...node.spec, ...editable, displayName: name,
      inputRef: inputNodeId ? { name: inputNodeId } : null, settings: { [node.spec.extensionRef ? "custom" : type]: node.spec.extensionRef ? settings : this.settingsForInput(type, settings, inputNodeId) } }
  }

  /** Select the display's broadcast endpoint from the existing input control. */
  private settingsForInput(type: NodeRole, settings: NodeUpdate["settings"], inputNodeId: string | null): NodeUpdate["settings"] {
    if (type !== "display") return settings
    const input = inputNodeId ? this.nodeById.get(inputNodeId) : undefined
    return { ...settings, inputStream: input?.type === "transcriber" ? "transcription" : "translation" }
  }

  private upsertNode(node: Node): void {
    const index = this.items.findIndex((item) => item.id === node.id)
    if (index === -1) {
      this.items = [...this.items, node]
      this.total += 1
    } else {
      this.items = this.items.map((item) => item.id === node.id ? node : item)
    }
  }
}
