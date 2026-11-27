/** Direct and inherited board roles as carried by membership responses. */
export type MemberRole = 'viewer' | 'commenter' | 'editor' | 'admin' | '';

export interface ContainerMember {
  containerId: string;
  userId: string;
  role: Exclude<MemberRole, ''>;
  synthetic: boolean;
  inheritedRole?: Exclude<MemberRole, 'admin' | ''>;
}
