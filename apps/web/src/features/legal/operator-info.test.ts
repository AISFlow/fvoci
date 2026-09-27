import assert from "node:assert/strict";
import test from "node:test";
import {
  filledOperatorFields,
  hasOperatorInfo,
  operatorFieldHref,
  type OperatorInfo,
} from "./operator-fields.ts";

const full: OperatorInfo = {
  businessName: "Example Co",
  representative: "Kim",
  registrationNumber: "123-45-67890",
  mailOrderNumber: "2026-SEOUL-1",
  address: "Seoul",
  phone: "02-0000-0000",
  supportEmail: "support@example.com",
  businessInfoUrl: "https://www.ftc.go.kr/bizCommPop.do?wrkr_no=1",
  hostingProvider: "AWS",
};

test("filledOperatorFields omits null leaves", () => {
  const partial: OperatorInfo = {
    ...full,
    representative: null,
    supportEmail: null,
    businessInfoUrl: null,
  };
  assert.deepEqual(filledOperatorFields(partial), [
    "businessName",
    "registrationNumber",
    "mailOrderNumber",
    "address",
    "phone",
    "hostingProvider",
  ]);
  assert.equal(hasOperatorInfo(partial), true);
  assert.equal(hasOperatorInfo(null), false);
  assert.equal(hasOperatorInfo({}), false);
});

test("operatorFieldHref builds safe mailto and https links only", () => {
  assert.equal(
    operatorFieldHref("supportEmail", "support@example.com"),
    "mailto:support@example.com",
  );
  assert.equal(operatorFieldHref("supportEmail", "not-an-email"), null);
  assert.equal(
    operatorFieldHref("businessInfoUrl", "https://example.com/info"),
    "https://example.com/info",
  );
  assert.equal(operatorFieldHref("businessInfoUrl", "javascript:alert(1)"), null);
  assert.equal(operatorFieldHref("businessInfoUrl", "mailto:x@y.com"), null);
  assert.equal(operatorFieldHref("phone", "02-1234"), null);
});

test("hasOperatorInfo treats all-null operator as empty", () => {
  const empty: OperatorInfo = {
    businessName: null,
    representative: null,
    registrationNumber: null,
    mailOrderNumber: null,
    address: null,
    phone: null,
    supportEmail: null,
    businessInfoUrl: null,
    hostingProvider: null,
  };
  assert.equal(hasOperatorInfo(empty), false);
  assert.equal(filledOperatorFields(empty).length, 0);
});
