require('node:vm').runInNewContext(process.env.ZED_TYPESCRIPT_RESOLVER, { require, module, process });
