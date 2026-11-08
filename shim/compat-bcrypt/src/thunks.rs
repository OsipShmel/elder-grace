// Generated from the bcrypt.dll export list of Wine (GPTK 3.0-3), 61 functions.
// Every function is a naked trampoline: jmp to an address from the REAL table (filled in DllMain).
// For functions with a BCRYPT_ALG_HANDLE first argument the trampoline translates the pseudo-handle first.

pub const NAMES: [&str; 61] = [
    "BCryptAddContextFunction",
    "BCryptAddContextFunctionProvider",
    "BCryptCloseAlgorithmProvider",
    "BCryptConfigureContext",
    "BCryptConfigureContextFunction",
    "BCryptCreateContext",
    "BCryptCreateHash",
    "BCryptDecrypt",
    "BCryptDeleteContext",
    "BCryptDeriveKey",
    "BCryptDeriveKeyCapi",
    "BCryptDeriveKeyPBKDF2",
    "BCryptDestroyHash",
    "BCryptDestroyKey",
    "BCryptDestroySecret",
    "BCryptDuplicateHash",
    "BCryptDuplicateKey",
    "BCryptEncrypt",
    "BCryptEnumAlgorithms",
    "BCryptEnumContextFunctionProviders",
    "BCryptEnumContextFunctions",
    "BCryptEnumContexts",
    "BCryptEnumProviders",
    "BCryptEnumRegisteredProviders",
    "BCryptExportKey",
    "BCryptFinalizeKeyPair",
    "BCryptFinishHash",
    "BCryptFreeBuffer",
    "BCryptGenRandom",
    "BCryptGenerateKeyPair",
    "BCryptGenerateSymmetricKey",
    "BCryptGetFipsAlgorithmMode",
    "BCryptGetProperty",
    "BCryptHash",
    "BCryptHashData",
    "BCryptImportKey",
    "BCryptImportKeyPair",
    "BCryptOpenAlgorithmProvider",
    "BCryptQueryContextConfiguration",
    "BCryptQueryContextFunctionConfiguration",
    "BCryptQueryContextFunctionProperty",
    "BCryptQueryProviderRegistration",
    "BCryptRegisterConfigChangeNotify",
    "BCryptRegisterProvider",
    "BCryptRemoveContextFunction",
    "BCryptRemoveContextFunctionProvider",
    "BCryptResolveProviders",
    "BCryptSecretAgreement",
    "BCryptSetAuditingInterface",
    "BCryptSetContextFunctionProperty",
    "BCryptSetProperty",
    "BCryptSignHash",
    "BCryptUnregisterConfigChangeNotify",
    "BCryptUnregisterProvider",
    "BCryptVerifySignature",
    "GetAsymmetricEncryptionInterface",
    "GetCipherInterface",
    "GetHashInterface",
    "GetRngInterface",
    "GetSecretAgreementInterface",
    "GetSignatureInterface",
];

thunk!(plain, BCryptAddContextFunction, 0);
thunk!(plain, BCryptAddContextFunctionProvider, 1);
thunk!(plain, BCryptCloseAlgorithmProvider, 2);
thunk!(plain, BCryptConfigureContext, 3);
thunk!(plain, BCryptConfigureContextFunction, 4);
thunk!(plain, BCryptCreateContext, 5);
thunk!(alg_trampoline, BCryptCreateHash, 6);
thunk!(plain, BCryptDecrypt, 7);
thunk!(plain, BCryptDeleteContext, 8);
thunk!(plain, BCryptDeriveKey, 9);
thunk!(plain, BCryptDeriveKeyCapi, 10);
thunk!(alg_trampoline, BCryptDeriveKeyPBKDF2, 11);
thunk!(plain, BCryptDestroyHash, 12);
thunk!(plain, BCryptDestroyKey, 13);
thunk!(plain, BCryptDestroySecret, 14);
thunk!(plain, BCryptDuplicateHash, 15);
thunk!(plain, BCryptDuplicateKey, 16);
thunk!(plain, BCryptEncrypt, 17);
thunk!(plain, BCryptEnumAlgorithms, 18);
thunk!(plain, BCryptEnumContextFunctionProviders, 19);
thunk!(plain, BCryptEnumContextFunctions, 20);
thunk!(plain, BCryptEnumContexts, 21);
thunk!(plain, BCryptEnumProviders, 22);
thunk!(plain, BCryptEnumRegisteredProviders, 23);
thunk!(plain, BCryptExportKey, 24);
thunk!(plain, BCryptFinalizeKeyPair, 25);
thunk!(plain, BCryptFinishHash, 26);
thunk!(plain, BCryptFreeBuffer, 27);
thunk!(alg_trampoline, BCryptGenRandom, 28);
thunk!(alg_trampoline, BCryptGenerateKeyPair, 29);
thunk!(alg_trampoline, BCryptGenerateSymmetricKey, 30);
thunk!(plain, BCryptGetFipsAlgorithmMode, 31);
thunk!(alg_trampoline, BCryptGetProperty, 32);
thunk!(alg_trampoline, BCryptHash, 33);
thunk!(plain, BCryptHashData, 34);
thunk!(alg_trampoline, BCryptImportKey, 35);
thunk!(alg_trampoline, BCryptImportKeyPair, 36);
thunk!(plain, BCryptOpenAlgorithmProvider, 37);
thunk!(plain, BCryptQueryContextConfiguration, 38);
thunk!(plain, BCryptQueryContextFunctionConfiguration, 39);
thunk!(plain, BCryptQueryContextFunctionProperty, 40);
thunk!(plain, BCryptQueryProviderRegistration, 41);
thunk!(plain, BCryptRegisterConfigChangeNotify, 42);
thunk!(plain, BCryptRegisterProvider, 43);
thunk!(plain, BCryptRemoveContextFunction, 44);
thunk!(plain, BCryptRemoveContextFunctionProvider, 45);
thunk!(plain, BCryptResolveProviders, 46);
thunk!(plain, BCryptSecretAgreement, 47);
thunk!(plain, BCryptSetAuditingInterface, 48);
thunk!(plain, BCryptSetContextFunctionProperty, 49);
thunk!(alg_trampoline, BCryptSetProperty, 50);
thunk!(plain, BCryptSignHash, 51);
thunk!(plain, BCryptUnregisterConfigChangeNotify, 52);
thunk!(plain, BCryptUnregisterProvider, 53);
thunk!(plain, BCryptVerifySignature, 54);
thunk!(plain, GetAsymmetricEncryptionInterface, 55);
thunk!(plain, GetCipherInterface, 56);
thunk!(plain, GetHashInterface, 57);
thunk!(plain, GetRngInterface, 58);
thunk!(plain, GetSecretAgreementInterface, 59);
thunk!(plain, GetSignatureInterface, 60);
