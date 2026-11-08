import type { Extension, ExtensionSpec } from "../../src/stores/models"

const accessToken = "mock-token"
const ociManifest = "displayName: Hello\nadapter:\n  execution:\n    command: ./hello"

/** Build a resource whose API identity differs from its display name. */
function resource(spec?: Partial<ExtensionSpec>): Extension {
  return {
    name: "custom-captioner",
    displayName: "Custom captioner", sourceType: "local", source: "/opt/extensions/captioner", adapter: { inputType: "audio", outputType: "text", execution: { command: "./captioner", runtime: "executable" } }, ...spec,
  }
}

/** Visit with authentication and a spec-conforming resource list. */
function visitAuthenticatedExtensions(items?: Extension[]) {
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/extensions", items ? { body: { items, resourceVersion: "1" } } : { fixture: "api/v1/extensions.json" }).as("getExtensions")
  cy.visit("/extensions", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", accessToken) } })
  cy.wait(["@getCurrentUser", "@getExtensions"])
}

/** Keep list responses consistent with successful registrations during polling. */
function stubRegistration(extension: Extension) {
  cy.fixture("api/v1/extensions.json").then((list) => {
    cy.intercept("GET", "**/api/v1/extensions", (request) => request.reply(list))
    cy.intercept("POST", "**/api/v1/extensions", (request) => {
      list.items.push(extension)
      request.reply({ statusCode: 201, body: extension })
    }).as("registerExtension")
  })
}

describe("Extensions page", () => {
  it("uses the declared output type for each custom extension", () => {
    const types = [
      ["image", "image"],
      ["text", "text-align-start"],
      ["audio", "audio-waveform"],
    ] as const
    visitAuthenticatedExtensions(types.map(([outputType]) => ({
      ...resource({ displayName: `${outputType} extension`, adapter: { inputType: "audio", outputType, execution: { command: "./run" } } }),
      name: outputType,
    })))
    for (const [outputType, icon] of types) {
      cy.contains("tr", `${outputType} extension`).should("contain.text", `audio → ${outputType}`).find(`svg.lucide-${icon}`).should("be.visible")
    }
    cy.screenshot("extension-output-icons")
  })

  it("shows a source extension without an inputType or a dangling arrow", () => {
    const source = resource()
    delete source.adapter.inputType
    visitAuthenticatedExtensions([source])
    cy.contains("tr", source.displayName).within(() => {
      cy.get("svg.lucide-text-align-start").should("be.visible")
      cy.contains("span", "text").should("not.contain.text", "→")
      cy.root().should("not.contain.text", "undefined")
    })
  })

  for (const inputType of ["audio", undefined] as const) {
    it(`renders input ${inputType} and omitted output without a dangling arrow`, () => {
      const item = resource()
      item.adapter.inputType = inputType
      delete item.adapter.outputType
      visitAuthenticatedExtensions([item])
      cy.contains("tr", item.displayName).within(() => {
        cy.get(`svg.lucide-${inputType ? "audio-waveform" : "blocks"}`).should("be.visible")
        cy.root().should("not.contain.text", "→").and("not.contain.text", "undefined").and("not.contain.text", "null")
      })
    })
  }

  it("reads flat extensions and preserves resource status after an accepted refresh", () => {
    cy.fixture("api/v1/extensions.json").then((list) => {
      const refreshed = { ...list.items[0] }
      cy.intercept("POST", "**/api/v1/extensions/obs-captions/refresh", { statusCode: 202, body: refreshed }).as("refreshExtension")
    })
    visitAuthenticatedExtensions()
    cy.get("@getExtensions").its("response.body.items").then((items: Extension[]) => {
      for (const item of items) {
        expect(item).to.have.property("displayName")
        expect(item).to.have.property("name")
        for (const field of ["apiVersion", "kind", "metadata", "spec", "generation", "observedGeneration"]) expect(item).not.to.have.property(field)
      }
    })
    cy.contains("2 extensions").should("be.visible")
    cy.contains("tr", "obs-captions").within(() => {
      cy.contains("Published").should("be.visible")
      cy.get('button[aria-label="Actions for obs-captions"]').click()
    })
    cy.contains('[data-slot="dropdown-menu-item"]', "Refresh extension").click()
    cy.wait("@refreshExtension").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body ?? "").to.equal("")
    })
    cy.contains("tr", "obs-captions").should("contain.text", "Published")
    cy.contains("tr", "obs-captions").should("not.contain.text", "Previous artifact:")
  })

  for (const [sourceType, label, source] of [
    ["local", "Local", "/opt/extensions/captioner"],
    ["git", "Git", "https://github.com/example/captioner.git"],
    ["oci", "OCI", "registry.example.com/captioner:latest"],
    ["http", "HTTP", "https://example.com/hello.py"],
  ] as const) {
    it(`registers a ${label} source from its path or URL without manual manifest fields`, () => {
      visitAuthenticatedExtensions()
      stubRegistration(resource({ sourceType, source, adapter: undefined }))
      cy.contains("button", "Register extension").click()
      cy.get("#extension-name, #extension-command, #extension-bindings").should("not.exist")
      cy.get('input[type="file"]').should("not.exist")
      if (sourceType !== "local") {
        cy.get('button[aria-label="Source type"]').click()
        cy.contains('[data-slot="select-item"]', label).click()
      }
      cy.get("#extension-source").type(source)
      if (sourceType === "oci") {
        cy.contains('label', "OCI URL").should("be.visible")
        cy.contains('[data-slot="dialog-content"] button', "Register extension").should("be.disabled")
        cy.get("#extension-manifest").type(ociManifest, { parseSpecialCharSequences: false })
      } else cy.get("#extension-manifest").should("not.exist")
      cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
      cy.wait("@registerExtension").then(({ request }) => {
        expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
        expect(request.body).to.deep.equal({ sourceType, source, ...(sourceType === "oci" ? { manifest: ociManifest } : {}) })
      })
      cy.get('[data-slot="dialog-content"]').should("not.exist")
      cy.contains("tr", "Custom captioner").should("contain.text", "Pending").and("contain.text", "Resolving manifest")
      cy.contains("3 extensions").should("be.visible")
    })
  }

  it("retains OCI manifest contents on errors and clears them when switching sources", () => {
    cy.intercept("POST", "**/api/v1/extensions", { statusCode: 503, body: { error: { message: "Registration unavailable" } } }).as("registerOci")
    visitAuthenticatedExtensions()
    cy.contains("button", "Register extension").click()
    cy.get('button[aria-label="Source type"]').click()
    cy.contains('[data-slot="select-item"]', "OCI").click()
    cy.get("#extension-source").type("registry.example.com/hello:latest")
    cy.get("#extension-manifest").type('{"displayName":"Hello","adapter":{"execution":{"command":"./hello"}}}', { parseSpecialCharSequences: false })
    cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
    cy.wait("@registerOci").its("request.body.manifest").should("be.a", "string")
    cy.get("#extension-manifest").should("contain.value", "displayName")
    cy.get('[role="alert"]').should("contain.text", "Registration unavailable")
    cy.get('button[aria-label="Source type"]').click()
    cy.contains('[data-slot="select-item"]', "Git").click()
    cy.get("#extension-manifest").should("not.exist")
    cy.get('button[aria-label="Source type"]').click()
    cy.contains('[data-slot="select-item"]', "OCI").click()
    cy.get("#extension-manifest").should("have.value", "")
    cy.contains("button", "Cancel").click()
  })

  it("retains the source after registration errors and clears it on cancel", () => {
    cy.intercept("POST", "**/api/v1/extensions", { statusCode: 422, body: { error: { message: "Extension registration was rejected." } } }).as("registerExtension")
    visitAuthenticatedExtensions()
    cy.contains("button", "Register extension").click()
    cy.contains('[data-slot="dialog-content"] button', "Register extension").should("be.disabled")
    cy.get("#extension-source").type("/opt/suntitle/extensions/hello")
    cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
    cy.wait("@registerExtension")
    cy.get('[role="alert"]').should("contain.text", "Extension registration was rejected")
    cy.get("#extension-source").should("have.value", "/opt/suntitle/extensions/hello")
    cy.contains("button", "Cancel").click()
    cy.contains("button", "Register extension").click()
    cy.get("#extension-source").should("have.value", "")
    cy.get('[role="alert"]').should("not.exist")
  })

  it("disables registration while the request is being persisted", () => {
    cy.intercept("POST", "**/api/v1/extensions", { statusCode: 201, body: resource(), delay: 1500 }).as("registerExtension")
    visitAuthenticatedExtensions()
    cy.contains("button", "Register extension").click()
    cy.get("#extension-source").type("/opt/suntitle/extensions/hello")
    cy.contains('[data-slot="dialog-content"] button', "Register extension").click()
    cy.contains("button", "Registering…").should("be.disabled")
    cy.get("#extension-source").should("be.disabled")
    cy.wait("@registerExtension")
    cy.get('[data-slot="dialog-content"]').should("not.exist")
  })

  it("reconciles asynchronous publication and reports terminal failure", () => {
    cy.clock()
    visitAuthenticatedExtensions()
    cy.contains("2 extensions").should("be.visible")
    const extension = resource({ adapter: undefined })
    extension.status = { observedGeneration: 1, conditions: [{ type: "Ready", status: "False", reason: "PreparationFailed", message: "Manifest could not be read after three attempts", lastTransitionTime: "2026-11-06T12:00:00Z" }] }
    cy.intercept("GET", "**/api/v1/extensions", { body: { items: [extension], resourceVersion: "20" } }).as("reconcileExtensions")
    cy.tick(5000)
    cy.wait("@reconcileExtensions")
    cy.contains("tr", "Custom captioner").should("contain.text", "Failed").and("contain.text", "Manifest could not be read after three attempts")
    cy.get('button[aria-label="Actions for custom-captioner"]').should("be.visible")
  })
})
