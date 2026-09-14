import { createOpenAI } from "@ai-sdk/openai";
export function createE2EProvider(options) {
  const sdk = createOpenAI(options);
  return {
    ...sdk,
    languageModel: (id) => {
      const model = sdk.responses(id);
      for (const method of ["doGenerate", "doStream"]) {
        const original = model[method].bind(model);
        model[method] = (args) =>
          original({
            ...args,
            providerOptions: {
              ...args.providerOptions,
              openai: { ...args.providerOptions?.openai, store: false },
            },
          });
      }
      return model;
    },
  };
}
