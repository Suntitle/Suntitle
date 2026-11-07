import type { Extension, NodeResource } from "../../src/stores/models"

/** Provide an extension whose display name differs from its resource identity. */
function extension(settings: Extension["adapter"]["settings"] = {}, outputType: Extension["adapter"]["outputType"] = "text"): Extension {
  return {
    name: "hello",
    displayName: "Hello every 10 seconds", sourceType: "local", source: "/opt/suntitle/extensions/hello",
    adapter: { inputType: "audio", outputType, execution: { runtime: "python", command: "./hello.py" }, settings },
  }
}

/** Model persistence so navigation and subsequent settings edits read the created custom node. */
function openHost(item: Extension) {
  const nodes: NodeResource[] = []
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" })
  cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" }).as("hosts")
  cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" }).as("types")
  cy.intercept("GET", "**/api/v1/extensions", { body: { items: [item], resourceVersion: "1" } }).as("extensions")
  cy.intercept("GET", "**/api/v1/nodes", (request) => request.reply({ items: nodes, resourceVersion: "1" })).as("nodes")
  cy.intercept("POST", "**/api/v1/hosts/*/nodes", (request) => {
    const resource = { ...request.body, name: "hello-node" }
    nodes.push(resource)
    request.reply({ statusCode: 201, body: resource })
  }).as("install")
  cy.intercept("PUT", "**/api/v1/nodes/hello-node", (request) => {
    nodes[0] = { ...nodes[0], ...request.body }
    request.reply(nodes[0])
  }).as("save")
  cy.visit("/hosts", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", "mock-token") } })
  cy.wait("@hosts")
  cy.contains("button", "studio-host").click()
  cy.wait(["@types", "@extensions", "@nodes"])
  cy.get('button[aria-label="Node type"]').should("contain.text", "Transcriber").click()
  cy.get('[data-slot="select-content"]').filter(":visible").should("have.length", 1).within(() => {
    cy.contains('[data-slot="select-item"]', "Transcriber").should("be.visible")
    cy.contains('[data-slot="select-item"]', "Hello every 10 seconds · Extension").click()
  })
}

describe("Extension node types", () => {
  it("installs and edits a source extension with no inputType", () => {
    const source = extension({ endpoint: { default: "transcribe" } })
    source.adapter.arguments = [{ flag: "--server", url: { base: "runtime.websocketUrl", path: "settings.endpoint" } }]
    delete source.adapter.inputType
    openHost(source)
    cy.get('button[aria-label="Node type"]').should("contain.text", "Hello every 10 seconds")
    cy.get("#node-name").should("have.value", "hello")
    cy.get('button[aria-label="Input"]').should("not.exist")
    cy.get("#node-setting-websocketUrl").should("not.exist")
    cy.get("#node-setting-endpoint").should("have.value", "transcribe").clear().type("translate")
    cy.get("#node-setting-sourceLanguage, #node-setting-targetLanguage").should("not.exist")
    cy.contains("button", "Install node").click()
    cy.wait("@install").then(({ request }) => {
      expect(request.body).to.deep.equal({
        displayName: "hello", enabled: true, hostRef: { name: "studio-host-resource" }, inputRef: null,
        extensionRef: { name: "hello" }, settings: { custom: { endpoint: "translate" } },
      })
    })
    cy.get('[role="status"]').should("contain.text", "installation was requested")
    cy.contains("button", "Cancel").click()
    cy.get('a[href="/nodes"]').first().click()
    cy.get('[data-testid="topology-node"]').should("have.length", 1).and("contain.text", "hello")
    cy.get('button[aria-label^="Open settings for hello,"]').click()
    cy.get('[data-slot="dialog-title"]').should("have.text", "Update hello")
    cy.get("#node-setting-endpoint").should("have.value", "translate").clear().type("captions")
    cy.contains("button", "Save changes").click()
    cy.wait("@save").its("request.body").should("include", { enabled: true })
    cy.get("@save").its("request.body.settings").should("deep.equal", { custom: { endpoint: "captions" } })
    cy.get("@save").its("request.body.extensionRef").should("deep.equal", { name: "hello" })
  })

  it("uses manifest defaults and preserves arbitrary JSON custom settings", () => {
    cy.viewport(1440, 1000)
    const item = extension({ message: { default: "Hello" }, enabled: { default: false }, nullable: { default: null }, nested: { default: { values: [1, true] } }, token: { required: true } })
    item.adapter.arguments = [{ flag: "--enabled", source: "settings.enabled", mode: "flag" }]
    openHost(item)
    cy.get("#node-setting-message").should("have.value", "Hello").clear().type("Hi")
    cy.get("#node-setting-nullable").should("have.value", "null")
    cy.get("#node-setting-nested").should("have.value", '{"values":[1,true]}')
    cy.get("#node-setting-token").type('"secret"')
    cy.get("#node-setting-nested").clear().type("invalid")
    cy.contains("button", "Install node").click()
    cy.get('[role="alert"]').should("contain.text", "nested must be valid JSON")
    cy.get("#node-setting-nested").clear().type('{"values":[1,true]}', { parseSpecialCharSequences: false })
    cy.contains("button", "Install node").click()
    cy.wait("@install").its("request.body.settings").should("deep.equal", {
      custom: { message: "Hi", enabled: false, nullable: null, nested: { values: [1, true] }, token: "secret" },
    })
  })

  for (const [inputType, outputType, icon] of [
    ["audio", "image", "image"], ["audio", "text", "text-align-start"], ["audio", "audio", "audio-waveform"],
    ["audio", undefined, "audio-waveform"], [undefined, undefined, "blocks"],
  ] as const) {
    it(`installs and edits an extension with input ${inputType} and output ${outputType}`, () => {
      const item = extension({}, outputType)
      item.adapter.inputType = inputType
      if (outputType === undefined) delete item.adapter.outputType
      openHost(item)
      cy.get('button[aria-label="Input"]').should("not.exist")
      cy.get("#node-setting-sourceLanguage, #node-setting-targetLanguage").should("not.exist")
      cy.contains("button", "Install node").click()
      cy.wait("@install").its("request.body").should("include", { displayName: "hello" })
      cy.get("@install").its("request.body.extensionRef").should("deep.equal", { name: "hello" })
      cy.get("@install").its("request.body.settings").should("deep.equal", { custom: {} })
      cy.contains("button", "Cancel").click()
      cy.get('a[href="/nodes"]').first().click()
      cy.get('[data-testid="topology-node"]').should("have.length", 1).and("contain.text", "Hello every 10 seconds")
      cy.get('[data-testid="topology-node"]').find(`svg.lucide-${icon}`).should("be.visible")
      cy.get('button[aria-label^="Open settings for hello,"]').click()
      cy.get('[data-slot="dialog-title"]').should("have.text", "Update hello")
      cy.get('button[aria-label="Input"]').should("not.exist")
      cy.contains("button", "Save changes").click()
      cy.wait("@save").its("request.body.settings").should("deep.equal", { custom: {} })
    })
  }

  it("keeps built-in types available when extension loading fails", () => {
    cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" })
    cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" })
    cy.intercept("GET", "**/api/v1/nodes", { body: { items: [] } })
    cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" })
    cy.intercept("GET", "**/api/v1/extensions", { statusCode: 503, body: { error: { message: "Registry unavailable" } } })
    cy.visit("/hosts", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", "mock-token") } })
    cy.contains("button", "studio-host").click()
    cy.get('[role="alert"]').should("contain.text", "Unable to load extension types")
    cy.get('button[aria-label="Node type"]').click()
    cy.contains('[data-slot="select-item"]', "Audio source").click()
    cy.get('button[aria-label="Node type"]').should("contain.text", "Audio source")
  })
})
